"""Multi-target tracker.

Stitches per-tick ``Contact`` reports (which carry an unstable per-tick id)
into persistent ``Track`` objects with smoothed position and velocity. This
is the keystone of the tactical toolkit — see
``docs/design-decisions/sdk-tactical-toolkit.md`` §4.2.

Determinism note: this code runs in the bot process, not the server. It is
free to use ordering-sensitive data structures; replay determinism is the
server's concern.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Dict, List, Optional, Set, Tuple

from ..helpers import bearing_to, signed_bearing_delta
from ..protocol import Contact, ContactKind, ShipSpecs, WorldView


@dataclass
class Track:
    """A persistent target estimate maintained by :class:`Tracker`.

    ``track_id`` is stable across ticks (unlike ``Contact.id`` from the wire).
    ``pos`` is the tracker's best estimate for the *current* tick — either
    the most recent observation, or a dead-reckoned prediction when the
    target was only seen passively (or not at all this tick).
    ``observed_pos`` is the raw last-active measurement; bots that want to
    inspect measurement vs. prediction can compare the two. ``vel`` is in
    units per second.
    """

    track_id: int
    kind: ContactKind
    pos: Tuple[float, float]
    observed_pos: Tuple[float, float]
    vel: Tuple[float, float]
    last_seen_tick: int
    first_seen_tick: int
    last_active_tick: int
    confidence: float
    source: str  # "active" | "passive" | "dead_reckoned"


class Tracker:
    """Per-tick association + smoothing of contacts into stable tracks.

    Typical use::

        tracker = Tracker(welcome.ship_specs, tick_hz=welcome.tick_hz)
        ...
        def on_tick(self, view):
            tracks = self.tracker.update(view)
            ...
    """

    def __init__(
        self,
        specs: ShipSpecs,
        tick_hz: int = 10,
        *,
        active_gate: float = 60.0,
        passive_bearing_gate_deg: float = 20.0,
        velocity_alpha: float = 0.3,
        velocity_window_ticks: int = 10,
        staleness_ticks: int = 40,
    ) -> None:
        self._specs = specs
        self._dt = 1.0 / float(tick_hz)
        self._active_gate = float(active_gate)
        self._passive_bearing_gate_deg = float(passive_bearing_gate_deg)
        self._alpha = float(velocity_alpha)
        self._vel_window = max(2, int(velocity_window_ticks))
        self._stale = int(staleness_ticks)
        self._tracks: Dict[int, Track] = {}
        # Per-track observation history (active hits only) for windowed velocity.
        self._history: Dict[int, List[Tuple[int, Tuple[float, float]]]] = {}
        self._next_id: int = 1

    # -- Public API --------------------------------------------------------

    def update(self, view: WorldView) -> List[Track]:
        """Fold ``view.contacts`` into the track set and return current tracks.

        Returns tracks sorted by ``track_id`` for stable iteration. Spawning
        is restricted to active (range-bearing) contacts; passive bearing-only
        contacts can only fold into existing tracks.
        """
        tick = view.tick

        # 1. Partition contacts.
        active_contacts: List[Contact] = [c for c in view.contacts if c.range is not None]
        passive_contacts: List[Contact] = [c for c in view.contacts if c.range is None]

        # 2. Greedy associate active contacts using predicted-to-current pos.
        matched: Set[int] = set()
        for contact in active_contacts:
            best_id: Optional[int] = None
            best_d = self._active_gate
            for tid, track in self._tracks.items():
                if tid in matched:
                    continue
                pred = self._predict(track, tick)
                d = math.hypot(pred[0] - contact.pos[0], pred[1] - contact.pos[1])
                if d < best_d:
                    best_d = d
                    best_id = tid
            if best_id is not None:
                self._fold_active(best_id, contact, tick)
                matched.add(best_id)
            else:
                tid = self._spawn(contact, tick)
                matched.add(tid)

        # 3. Associate passive contacts; never spawn.
        me_pos = view.me.pos
        for contact in passive_contacts:
            best_id = None
            best_b = self._passive_bearing_gate_deg
            for tid, track in self._tracks.items():
                if tid in matched:
                    continue
                pred = self._predict(track, tick)
                pred_bearing = bearing_to(me_pos, pred)
                delta = abs(signed_bearing_delta(contact.bearing_deg, pred_bearing))
                if delta < best_b:
                    best_b = delta
                    best_id = tid
            if best_id is not None:
                self._fold_passive(best_id, contact, tick)
                matched.add(best_id)

        # 4. Refresh ``pos`` for tracks that didn't get an active fold this
        #    tick — bot-facing position should always be predicted to current.
        for track in self._tracks.values():
            if track.last_active_tick != tick:
                track.pos = self._predict(track, tick)
                if track.last_seen_tick != tick:
                    track.source = "dead_reckoned"

        # 5. Stale old tracks.
        # A negative elapsed (``tick`` ran backwards, e.g. a Monte-Carlo match
        # reset ``world.tick`` to 0 under a persisted connection) is treated as
        # stale too — otherwise a carried-over track has
        # ``tick - last_seen_tick < 0``, never prunes, and becomes an immortal
        # "ghost" contact in every subsequent match.
        stale_ids = [
            tid
            for tid, t in self._tracks.items()
            if not (0 <= tick - t.last_seen_tick <= self._stale)
        ]
        for tid in stale_ids:
            del self._tracks[tid]
            self._history.pop(tid, None)

        return self.tracks

    @property
    def tracks(self) -> List[Track]:
        """Current tracks, sorted by ``track_id`` for stable iteration."""
        return sorted(self._tracks.values(), key=lambda t: t.track_id)

    def get(self, track_id: int) -> Optional[Track]:
        return self._tracks.get(track_id)

    def reset(self) -> None:
        """Drop all tracks and history. Call between matches.

        Configuration (specs, gates, staleness window) is preserved; only the
        per-match track set, observation history, and id counter are cleared.
        In Monte-Carlo mode the same ``Tracker`` instance persists across
        back-to-back matches, so this must be invoked on ``game_start`` to avoid
        carrying stale contacts (and a desync'd id counter) forward.
        """
        self._tracks.clear()
        self._history.clear()
        self._next_id = 1

    # -- Internals ---------------------------------------------------------

    def _predict(self, track: Track, tick: int) -> Tuple[float, float]:
        """Dead-reckon ``track`` to ``tick`` using its last anchor + velocity."""
        ticks_elapsed = tick - track.last_active_tick
        if ticks_elapsed <= 0:
            return track.observed_pos
        dt = ticks_elapsed * self._dt
        return (
            track.observed_pos[0] + track.vel[0] * dt,
            track.observed_pos[1] + track.vel[1] * dt,
        )

    def _fold_active(self, track_id: int, contact: Contact, tick: int) -> None:
        track = self._tracks[track_id]
        history = self._history[track_id]
        history.append((tick, contact.pos))
        if len(history) > self._vel_window:
            history.pop(0)

        # Velocity over the oldest-to-newest baseline of the window — averaging
        # multiple ticks of motion knocks ±2u sensor noise down by ~window.
        if len(history) >= 2:
            old_tick, old_pos = history[0]
            dt = (tick - old_tick) * self._dt
            if dt > 0:
                inst_vx = (contact.pos[0] - old_pos[0]) / dt
                inst_vy = (contact.pos[1] - old_pos[1]) / dt
                if track.vel == (0.0, 0.0):
                    track.vel = (inst_vx, inst_vy)
                else:
                    track.vel = (
                        self._alpha * inst_vx + (1.0 - self._alpha) * track.vel[0],
                        self._alpha * inst_vy + (1.0 - self._alpha) * track.vel[1],
                    )

        track.observed_pos = contact.pos
        track.pos = contact.pos
        track.last_seen_tick = tick
        track.last_active_tick = tick
        track.confidence = contact.confidence
        track.source = "active"
        if track.kind == "unknown":
            track.kind = contact.kind

    def _fold_passive(self, track_id: int, contact: Contact, tick: int) -> None:
        track = self._tracks[track_id]
        track.last_seen_tick = tick
        track.confidence = contact.confidence
        track.source = "passive"
        # Bearing-only: do not modify observed_pos, pos, or vel.

    def _spawn(self, contact: Contact, tick: int) -> int:
        tid = self._next_id
        self._next_id += 1
        self._tracks[tid] = Track(
            track_id=tid,
            kind=contact.kind,
            pos=contact.pos,
            observed_pos=contact.pos,
            vel=(0.0, 0.0),
            last_seen_tick=tick,
            first_seen_tick=tick,
            last_active_tick=tick,
            confidence=contact.confidence,
            source="active",
        )
        self._history[tid] = [(tick, contact.pos)]
        return tid
