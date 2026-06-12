"""``TacticalBot`` — the Layer 3 orchestrator.

Wires :class:`Tracker`, :class:`Gunner`, :class:`Helm`, :class:`SensorPolicy`,
and :class:`Evader` together. Bot authors subclass and override
``decide(ctx) -> Intent``. The framework translates the intent into a wire
command in the documented preemption order::

    Evader > Helm wall-override > Intent > Gunner > SensorPolicy

See ``docs/design-decisions/sdk-tactical-toolkit.md`` §4.7.
"""

from __future__ import annotations

from typing import Optional

from ..bot import Bot
from ..helpers import distance
from ..protocol import Command, Welcome, WorldView
from .context import TacticalContext, ThreatList
from .evader import Evader
from .gunner import Gunner
from .helm import Helm
from .intent import Intent, IntentKind
from .sensor import AlwaysActive, SensorPolicy
from .tracker import Tracker


class TacticalBot(Bot):
    """Higher-level :class:`Bot`. Subclass and override :meth:`decide`."""

    # Subsystems — populated in ``on_welcome``.
    tracker: Optional[Tracker] = None
    gunner: Optional[Gunner] = None
    helm: Optional[Helm] = None
    evader: Optional[Evader] = None
    sensor_policy: SensorPolicy = AlwaysActive()

    def __init__(self) -> None:
        super().__init__()
        self.tracker = None
        self.gunner = None
        self.helm = None
        self.evader = None
        # sensor_policy stays as the class default unless the subclass overrides
        # it in __init__ or in on_tactical_welcome.
        self._patrol_corner = 0

    # -- Hooks for subclasses ---------------------------------------------

    def decide(self, ctx: TacticalContext) -> Intent:
        """Override me. Return an :class:`Intent` describing what to do this tick."""
        return Intent.hold()

    def on_tactical_welcome(self, welcome: Welcome) -> None:
        """Override to customize subsystems (e.g. swap in a custom Tracker).

        Default subsystems have already been constructed by the time this fires.
        """

    # -- Framework wiring -------------------------------------------------

    def on_welcome(self, welcome: Welcome) -> None:
        # ``run_async`` also sets ``self.welcome`` before this fires, but we
        # set it here too so ``TacticalBot`` is independently testable.
        self.welcome = welcome
        specs = welcome.ship_specs
        self.tracker = Tracker(specs, tick_hz=welcome.tick_hz)
        self.gunner = Gunner(specs)
        self.helm = Helm(
            specs,
            map_width=float(welcome.map.width),
            map_height=float(welcome.map.height),
        )
        if self.evader is None:
            self.evader = Evader()
        self.on_tactical_welcome(welcome)

    def on_game_start(
        self, tick: int, starting_position, starting_heading_deg: float
    ) -> None:
        """Reset all match-scoped tactical state at the start of each match.

        In Monte-Carlo mode the connection (and therefore this ``TacticalBot``
        instance) persists across many back-to-back matches; the server resets
        ``world.tick`` to 0 and sends only ``game_start`` — never a fresh
        ``welcome``. Without this reset the :class:`Tracker` carries tracks from
        the previous match whose ``last_seen_tick`` now exceeds the new ``tick``,
        producing immortal "ghost" contacts that wedge reactive bots into
        permanently engaging a phantom (saturated throttle/rudder).

        Connection/welcome-derived config (map size, ship specs, sensor policy
        choice, powerup loadout) is deliberately preserved — only per-match
        runtime state is cleared. Subclasses overriding this should call
        ``super().on_game_start(...)``.
        """
        if self.tracker is not None:
            self.tracker.reset()
        if self.evader is not None and hasattr(self.evader, "reset"):
            self.evader.reset()
        if hasattr(self.sensor_policy, "reset"):
            self.sensor_policy.reset()
        # Patrol cursor is match-scoped: a fresh match starts from corner 0.
        self._patrol_corner = 0
        # Gunner's only mutable state is its fire-cooldown counter (keyed off the
        # match tick); a reset to tick 0 makes it immediately fireable again, so
        # it needs no explicit reset.

    def on_tick(self, view: WorldView) -> Command:
        if (
            self.tracker is None
            or self.gunner is None
            or self.helm is None
            or self.evader is None
            or self.welcome is None
        ):
            return Command()

        tracks = self.tracker.update(view)
        threats = ThreatList(
            tracks=[t for t in tracks if t.kind == "ship"],
            me_pos=view.me.pos,
        )
        ctx = TacticalContext(
            view=view,
            me=view.me,
            specs=self.welcome.ship_specs,
            tracker=self.tracker,
            threats=threats,
            map_width=float(self.welcome.map.width),
            map_height=float(self.welcome.map.height),
        )

        # 1. Evader preempts everything.
        evade_cmd = self.evader.update(view)
        if evade_cmd is not None:
            evade_cmd.sensor_mode = self.sensor_policy.choose(view, self.tracker)
            return evade_cmd

        # 2. Player intent.
        intent = self.decide(ctx)

        if intent.kind == IntentKind.CUSTOM:
            cmd = intent.command if intent.command is not None else Command()
            return cmd

        cmd = self._intent_to_command(intent, ctx)

        # 3. Sensor overlay.
        cmd.sensor_mode = self.sensor_policy.choose(view, self.tracker)

        # 4. Gunner overlay.
        target = self._select_fire_target(intent, threats)
        if target is not None:
            self.gunner.attempt(cmd, view.me, target, view)

        return cmd

    # -- Internals --------------------------------------------------------

    def _intent_to_command(self, intent: Intent, ctx: TacticalContext) -> Command:
        assert self.helm is not None
        if intent.kind == IntentKind.HOLD:
            return Command(throttle=0.0, rudder=0.0)
        if intent.kind == IntentKind.ENGAGE and intent.target is not None:
            throttle, rudder = self.helm.steer_to_point(ctx.me, intent.target.pos)
            return Command(throttle=throttle, rudder=rudder)
        if intent.kind == IntentKind.RETREAT_TO and intent.point is not None:
            throttle, rudder = self.helm.steer_to_point(ctx.me, intent.point)
            return Command(throttle=throttle, rudder=rudder)
        if intent.kind == IntentKind.PATROL and intent.rect is not None:
            waypoint = self._patrol_waypoint(intent.rect, ctx)
            throttle, rudder = self.helm.steer_to_point(ctx.me, waypoint)
            return Command(throttle=throttle, rudder=rudder)
        return Command()

    @staticmethod
    def _select_fire_target(intent: Intent, threats: ThreatList):
        if intent.kind == IntentKind.ENGAGE and intent.target is not None:
            return intent.target
        if intent.kind == IntentKind.HOLD:
            return None
        return threats.nearest()

    def _patrol_waypoint(self, rect, ctx: TacticalContext):
        x1, y1, x2, y2 = rect
        corners = [(x1, y1), (x2, y1), (x2, y2), (x1, y2)]
        target = corners[self._patrol_corner]
        if distance(ctx.me.pos, target) < 25.0:
            self._patrol_corner = (self._patrol_corner + 1) % 4
            target = corners[self._patrol_corner]
        return target
