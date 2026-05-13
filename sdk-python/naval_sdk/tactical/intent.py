"""High-level tactical intents.

An :class:`Intent` is what a player's ``decide()`` returns from a
:class:`TacticalBot`. The orchestrator translates the intent into a concrete
``Command`` via the L2 subsystems.

See ``docs/design-decisions/sdk-tactical-toolkit.md`` §4.7.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import TYPE_CHECKING, Optional, Tuple

from ..protocol import Command

if TYPE_CHECKING:
    from .tracker import Track


class IntentKind(Enum):
    ENGAGE = "engage"
    PATROL = "patrol"
    RETREAT_TO = "retreat_to"
    HOLD = "hold"
    CUSTOM = "custom"


@dataclass(frozen=True)
class Intent:
    """Tagged union of high-level commands. Construct via the named helpers."""

    kind: IntentKind
    target: Optional["Track"] = None
    rect: Optional[Tuple[float, float, float, float]] = None
    point: Optional[Tuple[float, float]] = None
    command: Optional[Command] = None

    @staticmethod
    def engage(target: "Track") -> "Intent":
        return Intent(IntentKind.ENGAGE, target=target)

    @staticmethod
    def patrol(rect: Tuple[float, float, float, float]) -> "Intent":
        """Patrol the corners of an axis-aligned rectangle ``(x1, y1, x2, y2)``."""
        return Intent(IntentKind.PATROL, rect=rect)

    @staticmethod
    def retreat_to(point: Tuple[float, float]) -> "Intent":
        return Intent(IntentKind.RETREAT_TO, point=point)

    @staticmethod
    def hold() -> "Intent":
        return Intent(IntentKind.HOLD)

    @staticmethod
    def custom(cmd: Command) -> "Intent":
        """Escape hatch: hand the orchestrator a raw ``Command`` for one tick."""
        return Intent(IntentKind.CUSTOM, command=cmd)
