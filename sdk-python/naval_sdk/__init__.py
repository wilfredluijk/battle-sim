"""Python SDK for the battle-sim naval server.

Public API:
    Bot           — base class to subclass
    Command       — per-tick action returned from `on_tick`
    WorldView     — typed view of a `tick` message
    Contact       — single contact in a `WorldView`
    SelfState     — own-ship state in a `WorldView`
    ShipSpecs     — gameplay constants from the `welcome` message
    GameOver      — final-message payload
    run           — connect a bot to a server and run to completion (sync wrapper)
    run_async     — same as `run`, but as an awaitable coroutine
    bearing_to    — math helper: compass bearing from one point to another
    distance      — math helper: Euclidean distance
    lead_target   — math helper: predicted intercept point for a moving target
"""

from .helpers import bearing_to, distance, lead_target
from .protocol import (
    Command,
    Contact,
    FireCommand,
    GameOver,
    SelfState,
    ShipSpecs,
    WorldView,
)
from .bot import Bot, run, run_async

__all__ = [
    "Bot",
    "Command",
    "Contact",
    "FireCommand",
    "GameOver",
    "SelfState",
    "ShipSpecs",
    "WorldView",
    "bearing_to",
    "distance",
    "lead_target",
    "run",
    "run_async",
]

__version__ = "0.1.0"
