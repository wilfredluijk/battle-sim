"""Tests for ``naval_sdk.tactical.gunner.Gunner``."""

from __future__ import annotations

from typing import List, Tuple

import pytest

from naval_sdk.protocol import (
    Command,
    Contact,
    SelfState,
    ShipSpecs,
    WorldView,
)
from naval_sdk.tactical import Gunner
from naval_sdk.tactical.tracker import Track

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


def _me(pos=(0.0, 0.0), ammo=20):
    return SelfState(pos=pos, heading_deg=0.0, speed=0.0, hp=100, ammo=ammo,
                     rudder=0.0, throttle=0.0)


def _view(tick: int, me: SelfState):
    return WorldView(tick=tick, deadline_ms=80, self_state=me, contacts=[], events=[])


def _track(track_id=1, pos=(100.0, 0.0), vel=(0.0, 0.0), last_active=0):
    return Track(
        track_id=track_id,
        kind="ship",
        pos=pos,
        observed_pos=pos,
        vel=vel,
        last_seen_tick=last_active,
        first_seen_tick=last_active,
        last_active_tick=last_active,
        confidence=1.0,
        source="active",
    )


def test_gunner_solves_at_rest():
    g = Gunner(SPECS)
    sol = g.solve(_me(), _track(pos=(100.0, 0.0)), _view(0, _me()))
    assert sol is not None
    assert sol.bearing_deg == pytest.approx(90.0)  # east
    assert sol.range == pytest.approx(100.0)


def test_gunner_respects_cooldown():
    g = Gunner(SPECS)
    me = _me()
    sol = g.solve(me, _track(), _view(0, me))
    assert sol is not None
    g.note_fired(0)
    # Cooldown is 15 ticks. Within window, solve must refuse.
    for tick in range(1, 15):
        assert g.solve(me, _track(last_active=tick), _view(tick, me)) is None
    # Tick 15: ok again.
    assert g.solve(me, _track(last_active=15), _view(15, me)) is not None


def test_gunner_refuses_when_out_of_range():
    g = Gunner(SPECS)
    me = _me()
    far_track = _track(pos=(SPECS.max_shell_range + 50.0, 0.0))
    assert g.solve(me, far_track, _view(0, me)) is None


def test_gunner_refuses_self_splash():
    g = Gunner(SPECS, self_splash_margin=1.5)
    me = _me()
    # 10 units away is well inside splash (radius 15 * margin 1.5 = 22.5).
    too_close = _track(pos=(10.0, 0.0))
    assert g.solve(me, too_close, _view(0, me)) is None


def test_gunner_refuses_stale_active_track():
    g = Gunner(SPECS, max_active_age_ticks=5)
    me = _me()
    # Track last fixed at tick 0; current tick is 10 -> too old.
    stale = _track(last_active=0)
    assert g.solve(me, stale, _view(10, me)) is None


def test_gunner_refuses_when_out_of_ammo():
    g = Gunner(SPECS)
    me = _me(ammo=0)
    assert g.solve(me, _track(), _view(0, me)) is None


def test_gunner_attempt_attaches_fire_and_starts_cooldown():
    g = Gunner(SPECS)
    me = _me()
    cmd = Command()
    fired = g.attempt(cmd, me, _track(), _view(0, me))
    assert fired
    assert cmd.fire is not None
    assert g.next_fire_tick == SPECS.gun_cooldown_ticks
    # Immediate retry refuses
    cmd2 = Command()
    assert not g.attempt(cmd2, me, _track(last_active=1), _view(1, me))
    assert cmd2.fire is None


def test_gunner_leads_moving_target():
    g = Gunner(SPECS)
    me = _me(pos=(0.0, 0.0))
    # Target at (100, 0) moving +y at 10 -> shooter must aim ahead.
    moving = _track(pos=(100.0, 0.0), vel=(0.0, 10.0))
    sol = g.solve(me, moving, _view(0, me))
    assert sol is not None
    # Aim point y should be > 0 (we lead in +y direction).
    assert sol.aim_pos[1] > 0.0
    # Bearing should be somewhere between east (90°) and south (180°).
    assert 90.0 < sol.bearing_deg < 180.0
