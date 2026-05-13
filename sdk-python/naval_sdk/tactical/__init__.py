"""Opt-in tactical toolkit for naval-sdk bot authors.

See ``docs/design-decisions/sdk-tactical-toolkit.md`` for the motivation. In
short: each class in this package is a self-contained subsystem you can drop
into a custom ``Bot.on_tick`` (Layer 2 of the SDK), or you can subclass
``TacticalBot`` and override ``decide()`` (Layer 3).

Nothing here is required. The base ``Bot``/``Command``/``WorldView`` API is
unchanged.
"""

from .tracker import Track, Tracker
from .gunner import FireSolution, Gunner
from .helm import Helm
from .sensor import (
    AlwaysActive,
    AlwaysPassive,
    DutyCycle,
    PingWhenStale,
    SensorPolicy,
)
from .evader import Evader, EvaderState
from .context import TacticalContext, ThreatList
from .intent import Intent, IntentKind
from .bot import TacticalBot

__all__ = [
    "AlwaysActive",
    "AlwaysPassive",
    "DutyCycle",
    "Evader",
    "EvaderState",
    "FireSolution",
    "Gunner",
    "Helm",
    "Intent",
    "IntentKind",
    "PingWhenStale",
    "SensorPolicy",
    "TacticalBot",
    "TacticalContext",
    "ThreatList",
    "Track",
    "Tracker",
]
