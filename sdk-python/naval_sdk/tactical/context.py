"""Context object passed to ``TacticalBot.decide()``.

Bundles the parsed wire view, the player's smoothed tracks, and a handy
``ThreatList`` of ship-kind tracks pre-sorted for tactical queries.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterator, List, Optional, Tuple

from ..helpers import distance
from ..protocol import SelfState, ShipSpecs, WorldView
from .tracker import Track, Tracker


@dataclass
class ThreatList:
    """Iterable view of threat tracks with cheap tactical queries."""

    tracks: List[Track]
    me_pos: Tuple[float, float]

    def __iter__(self) -> Iterator[Track]:
        return iter(self.tracks)

    def __len__(self) -> int:
        return len(self.tracks)

    def __bool__(self) -> bool:
        return bool(self.tracks)

    def nearest(self) -> Optional[Track]:
        if not self.tracks:
            return None
        return min(self.tracks, key=lambda t: distance(self.me_pos, t.pos))

    def farthest(self) -> Optional[Track]:
        if not self.tracks:
            return None
        return max(self.tracks, key=lambda t: distance(self.me_pos, t.pos))

    def by_id(self, track_id: int) -> Optional[Track]:
        for t in self.tracks:
            if t.track_id == track_id:
                return t
        return None


@dataclass
class TacticalContext:
    """All the state ``decide()`` should ever need."""

    view: WorldView
    me: SelfState
    specs: ShipSpecs
    tracker: Tracker
    threats: ThreatList
    map_width: float
    map_height: float
