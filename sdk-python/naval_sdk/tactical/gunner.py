"""Fire-control helper.

Wraps cooldown tracking, ammo accounting, lead-target computation, range and
time-of-flight feasibility, and a self-splash guard. ``Gunner.solve()`` is
pure (no side effects); callers call ``note_fired()`` when they actually
attach the resulting :class:`FireCommand` to their outbound command.

See ``docs/design-decisions/sdk-tactical-toolkit.md`` §4.3.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Optional, Tuple

from ..helpers import bearing_to, lead_target
from ..protocol import Command, FireCommand, SelfState, ShipSpecs, WorldView
from .tracker import Track


@dataclass(frozen=True)
class FireSolution:
    """A vetted firing solution ready to be sent on the wire."""

    bearing_deg: float
    range: float
    aim_pos: Tuple[float, float]
    target_id: int


class Gunner:
    def __init__(
        self,
        specs: ShipSpecs,
        *,
        self_splash_margin: float = 1.5,
        max_active_age_ticks: int = 5,
        require_recent_active: bool = True,
    ) -> None:
        """``self_splash_margin`` is a multiple of the ship's splash radius.

        ``max_active_age_ticks`` is how recent the last *active* (range-fixed)
        observation must be for the gunner to trust the position estimate.
        Pure-passive (dead-reckoned) tracks are skipped when
        ``require_recent_active`` is True (the default).
        """
        self._specs = specs
        self._cooldown = int(specs.gun_cooldown_ticks)
        self._self_splash = float(specs.splash_radius) * float(self_splash_margin)
        self._max_age = int(max_active_age_ticks)
        self._require_active = bool(require_recent_active)
        self._next_fire_tick = 0

    # -- Public API --------------------------------------------------------

    def solve(self, me: SelfState, track: Track, view: WorldView) -> Optional[FireSolution]:
        """Return a vetted :class:`FireSolution`, or ``None`` if no shot is available.

        Side-effect-free. Call :meth:`note_fired` if the caller actually fires.
        """
        if view.tick < self._next_fire_tick:
            return None
        if me.ammo <= 0:
            return None
        if self._require_active and (view.tick - track.last_active_tick) > self._max_age:
            return None

        # Intercept solution: `pred` is where the target will be when a shell fired
        # *now* reaches it. `lead_target` returns None when no real intercept exists
        # (target outruns the shell and is fleeing).
        pred = lead_target(me.pos, track.pos, track.vel, self._specs.shell_speed)
        if pred is None:
            return None

        # Range gate doubles as the time-of-flight feasibility check: by construction
        # |pred - me| == shell_speed * t, so requiring that distance <= max_shell_range
        # rejects shots where the target will have left range by the time the shell lands.
        rng = math.hypot(pred[0] - me.pos[0], pred[1] - me.pos[1])
        if rng > self._specs.max_shell_range:
            return None
        if rng < self._self_splash:
            return None

        bearing = bearing_to(me.pos, pred)
        return FireSolution(
            bearing_deg=bearing,
            range=rng,
            aim_pos=pred,
            target_id=track.track_id,
        )

    def attempt(self, cmd: Command, me: SelfState, track: Track, view: WorldView) -> bool:
        """Convenience: solve and attach to ``cmd``, recording cooldown.

        Returns ``True`` if a shot was attached.
        """
        sol = self.solve(me, track, view)
        if sol is None:
            return False
        cmd.fire = self.to_fire_command(sol)
        self.note_fired(view.tick)
        return True

    @staticmethod
    def to_fire_command(solution: FireSolution) -> FireCommand:
        return FireCommand(bearing_deg=solution.bearing_deg, range=solution.range)

    def note_fired(self, tick: int) -> None:
        """Record that a shot was committed at ``tick``; starts the cooldown."""
        self._next_fire_tick = tick + self._cooldown

    @property
    def next_fire_tick(self) -> int:
        return self._next_fire_tick

    def can_fire(self, view: WorldView, me: SelfState) -> bool:
        """Cheap pre-check: cooldown elapsed and ammo > 0."""
        return view.tick >= self._next_fire_tick and me.ammo > 0
