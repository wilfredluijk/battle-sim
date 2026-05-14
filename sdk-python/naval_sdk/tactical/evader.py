"""Hit-triggered evasive maneuver state machine.

Returns an override :class:`Command` while evading; ``None`` otherwise. The
orchestrator (:class:`TacticalBot`) treats a non-``None`` return as a
preempting command — evasion outranks the player's tactical intent.

See ``docs/design-decisions/sdk-tactical-toolkit.md`` §4.6.
"""

from __future__ import annotations

from enum import Enum
from typing import Optional

from ..protocol import Command, HitEvent, WorldView


class EvaderState(Enum):
    IDLE = "idle"
    EVADING = "evading"
    COOLDOWN = "cooldown"


class Evader:
    def __init__(
        self,
        *,
        evasion_ticks: int = 15,
        cooldown_ticks: int = 10,
        throttle: float = 1.0,
        initial_rudder_sign: float = 1.0,
    ) -> None:
        self._evasion_ticks = int(evasion_ticks)
        self._cooldown_ticks = int(cooldown_ticks)
        self._throttle = float(throttle)
        self._state: EvaderState = EvaderState.IDLE
        self._state_until: int = 0
        self._rudder_sign: float = 1.0 if initial_rudder_sign >= 0 else -1.0

    @property
    def state(self) -> EvaderState:
        return self._state

    def update(self, view: WorldView) -> Optional[Command]:
        """Advance the state machine. Returns an override ``Command`` while
        evading, else ``None``.
        """
        # Tick state transitions.
        if self._state != EvaderState.IDLE and view.tick >= self._state_until:
            if self._state == EvaderState.EVADING:
                self._state = EvaderState.COOLDOWN
                self._state_until = view.tick + self._cooldown_ticks
            else:
                self._state = EvaderState.IDLE

        # React to fresh hits.
        was_hit = any(isinstance(e, HitEvent) for e in view.events)
        if was_hit and self._state in (EvaderState.IDLE, EvaderState.COOLDOWN):
            if self._state == EvaderState.COOLDOWN:
                # Took another hit before we got clear — flip rudder to confuse aim.
                self._rudder_sign = -self._rudder_sign
            self._state = EvaderState.EVADING
            self._state_until = view.tick + self._evasion_ticks

        if self._state == EvaderState.EVADING:
            return Command(
                throttle=self._throttle,
                rudder=self._rudder_sign,
            )
        return None
