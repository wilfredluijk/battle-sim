"""Tests for ``naval_sdk.tactical.tracker.Tracker``."""

from __future__ import annotations

import random
from typing import List, Optional, Tuple

import pytest

from naval_sdk.helpers import bearing_to
from naval_sdk.protocol import (
    Contact,
    SelfState,
    ShipSpecs,
    WorldView,
)
from naval_sdk.tactical import Tracker

SPECS = ShipSpecs(
    max_forward_speed=6.0,
    max_reverse_speed=2.0,
    acceleration=1.5,
    turn_rate_deg_per_s=15.0,
    hull_hp=100,
    max_ammo=20,
    gun_cooldown_ticks=15,
    hit_radius=2.0,
    shell_speed=50.0,
    max_shell_range=300.0,
    splash_radius=15.0,
    max_splash_damage=25,
)


def _view(tick: int, me_pos: Tuple[float, float], contacts: List[Contact]) -> WorldView:
    me = SelfState(
        pos=me_pos,
        heading_deg=0.0,
        speed=0.0,
        hp=100,
        ammo=20,
        rudder=0.0,
        throttle=0.0,
    )
    return WorldView(tick=tick, deadline_ms=80, self_state=me, contacts=contacts, events=[])


def _active_contact(
    pos: Tuple[float, float], me_pos: Tuple[float, float] = (0.0, 0.0)
) -> Contact:
    rng = ((pos[0] - me_pos[0]) ** 2 + (pos[1] - me_pos[1]) ** 2) ** 0.5
    return Contact(
        id="x",
        kind="ship",
        pos=pos,
        bearing_deg=bearing_to(me_pos, pos),
        range=rng,
        confidence=1.0,
    )


def _passive_contact(bearing: float) -> Contact:
    return Contact(
        id="p",
        kind="ship",
        pos=(0.0, 0.0),
        bearing_deg=bearing,
        range=None,
        confidence=0.5,
    )


def test_tracker_associates_moving_target_with_noise():
    """Active contacts on a straight-line mover get stitched into one stable track."""

    tracker = Tracker(SPECS, tick_hz=10)
    rng = random.Random(42)

    # Target moves east at 5 u/s, starting at (100, 100). Ours sits at (0, 0).
    me_pos = (0.0, 0.0)
    base_pos = (100.0, 100.0)
    vel = (5.0, 0.0)

    track_ids = set()
    for tick in range(30):
        truth = (base_pos[0] + vel[0] * tick * 0.1, base_pos[1] + vel[1] * tick * 0.1)
        noisy = (truth[0] + rng.uniform(-2.0, 2.0), truth[1] + rng.uniform(-2.0, 2.0))
        view = _view(tick, me_pos, [_active_contact(noisy, me_pos)])
        tracks = tracker.update(view)
        assert len(tracks) == 1, f"tick {tick}: expected 1 track, got {len(tracks)}"
        track_ids.add(tracks[0].track_id)

    # Track ID must remain stable across all 30 ticks.
    assert len(track_ids) == 1, f"expected stable id, got {track_ids}"

    final = tracker.tracks[0]
    # Smoothed velocity should be close to (5, 0). Tolerate noise-induced drift.
    assert abs(final.vel[0] - 5.0) < 2.0
    assert abs(final.vel[1]) < 2.0


def test_tracker_folds_passive_into_existing_active_track():
    """A bearing-only contact along a known track's bearing folds into the track."""

    tracker = Tracker(SPECS, tick_hz=10, passive_bearing_gate_deg=20.0)
    me_pos = (0.0, 0.0)

    # Seed with two active hits to establish position and velocity.
    tracker.update(_view(0, me_pos, [_active_contact((100.0, 0.0), me_pos)]))
    tracker.update(_view(1, me_pos, [_active_contact((105.0, 0.0), me_pos)]))
    assert len(tracker.tracks) == 1
    track_id = tracker.tracks[0].track_id
    last_active_tick = tracker.tracks[0].last_active_tick

    # Now a passive-only contact at roughly the same bearing (east = 90°).
    passive = _passive_contact(bearing=92.0)
    tracker.update(_view(2, me_pos, [passive]))

    assert len(tracker.tracks) == 1
    track = tracker.tracks[0]
    assert track.track_id == track_id  # same track, folded
    assert track.last_seen_tick == 2
    assert track.last_active_tick == last_active_tick  # not updated by passive
    assert track.source == "passive"


def test_tracker_stales_unseen_tracks():
    tracker = Tracker(SPECS, tick_hz=10, staleness_ticks=5)
    me_pos = (0.0, 0.0)

    tracker.update(_view(0, me_pos, [_active_contact((100.0, 0.0), me_pos)]))
    assert len(tracker.tracks) == 1

    # No contacts for 6 ticks -> dropped.
    for tick in range(1, 7):
        tracker.update(_view(tick, me_pos, []))
    assert tracker.tracks == []


def test_tracker_does_not_spawn_from_passive_only():
    """Passive-only contacts with no existing track to fold into are discarded."""
    tracker = Tracker(SPECS, tick_hz=10)
    me_pos = (0.0, 0.0)
    tracker.update(_view(0, me_pos, [_passive_contact(bearing=90.0)]))
    assert tracker.tracks == []


def test_tracker_spawns_separate_tracks_for_distant_contacts():
    tracker = Tracker(SPECS, tick_hz=10, active_gate=60.0)
    me_pos = (0.0, 0.0)
    view = _view(
        0,
        me_pos,
        [
            _active_contact((100.0, 0.0), me_pos),
            _active_contact((-100.0, 0.0), me_pos),
        ],
    )
    tracks = tracker.update(view)
    assert len(tracks) == 2
    assert {t.track_id for t in tracks} == {1, 2}
