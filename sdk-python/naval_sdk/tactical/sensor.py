"""Sensor mode policies.

A :class:`SensorPolicy` decides whether to broadcast active radar (full
position fixes, but pingable by passive listeners) or to listen passively
(bearing-only, stealthier) on a given tick. Implementations are tiny — the
intent here is to make the *decision* explicit rather than re-implement
duty cycles in every bot.

See ``docs/design-decisions/sdk-tactical-toolkit.md`` §4.5.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Protocol

from ..protocol import SensorMode, WorldView

if TYPE_CHECKING:
    from .tracker import Tracker


class SensorPolicy(Protocol):
    """Pluggable interface. Implement ``choose()`` and pass into ``TacticalBot``."""

    def choose(self, view: WorldView, tracker: "Tracker") -> SensorMode: ...


class AlwaysActive:
    def choose(self, view: WorldView, tracker: "Tracker") -> SensorMode:
        return "active"


class AlwaysPassive:
    def choose(self, view: WorldView, tracker: "Tracker") -> SensorMode:
        return "passive"


@dataclass
class DutyCycle:
    """Cycle: ``active_ticks`` of active, then ``passive_ticks`` of passive."""

    active_ticks: int = 10
    passive_ticks: int = 20

    def choose(self, view: WorldView, tracker: "Tracker") -> SensorMode:
        cycle = max(1, self.active_ticks + self.passive_ticks)
        phase = view.tick % cycle
        return "active" if phase < self.active_ticks else "passive"


@dataclass
class PingWhenStale:
    """Active only when no track has a fresh fix.

    Specifically: passive while ``min(tick - last_seen) < threshold`` for the
    track set; active otherwise (including when the track set is empty).
    """

    stale_threshold_ticks: int = 15

    def choose(self, view: WorldView, tracker: "Tracker") -> SensorMode:
        tracks = tracker.tracks
        if not tracks:
            return "active"
        freshest_gap = min(view.tick - t.last_seen_tick for t in tracks)
        return "active" if freshest_gap >= self.stale_threshold_ticks else "passive"
