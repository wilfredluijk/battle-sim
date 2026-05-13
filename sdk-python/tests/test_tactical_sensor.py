"""Tests for ``naval_sdk.tactical.sensor`` policies."""

from __future__ import annotations

from typing import List

from naval_sdk.protocol import Contact, SelfState, ShipSpecs, WorldView
from naval_sdk.tactical import (
    AlwaysActive,
    AlwaysPassive,
    DutyCycle,
    PingWhenStale,
    Tracker,
)

SPECS = ShipSpecs(
    max_forward_speed=6.0, max_reverse_speed=2.0, acceleration=1.5,
    turn_rate_deg_per_s=15.0, hull_hp=100, max_ammo=20, gun_cooldown_ticks=15,
    hit_radius=2.0, shell_speed=50.0, max_shell_range=300.0,
    splash_radius=15.0, max_splash_damage=25,
)


def _view(tick: int, contacts: List[Contact] = None) -> WorldView:
    me = SelfState(pos=(0.0, 0.0), heading_deg=0.0, speed=0.0, hp=100, ammo=20,
                   rudder=0.0, throttle=0.0)
    return WorldView(tick=tick, deadline_ms=80, self_state=me,
                     contacts=contacts or [], events=[])


def _ranged_contact(pos=(100.0, 0.0)) -> Contact:
    return Contact(id="x", kind="ship", pos=pos, bearing_deg=90.0,
                   range=100.0, confidence=1.0)


def test_always_active():
    pol = AlwaysActive()
    tr = Tracker(SPECS)
    assert pol.choose(_view(0), tr) == "active"
    assert pol.choose(_view(99), tr) == "active"


def test_always_passive():
    pol = AlwaysPassive()
    tr = Tracker(SPECS)
    assert pol.choose(_view(0), tr) == "passive"


def test_duty_cycle_sequence():
    pol = DutyCycle(active_ticks=3, passive_ticks=2)
    tr = Tracker(SPECS)
    modes = [pol.choose(_view(t), tr) for t in range(10)]
    # 5-tick cycle: a,a,a,p,p,a,a,a,p,p
    assert modes == [
        "active", "active", "active", "passive", "passive",
        "active", "active", "active", "passive", "passive",
    ]


def test_ping_when_stale_active_with_no_tracks():
    pol = PingWhenStale(stale_threshold_ticks=10)
    tr = Tracker(SPECS)
    assert pol.choose(_view(0), tr) == "active"


def test_ping_when_stale_passive_when_fresh():
    pol = PingWhenStale(stale_threshold_ticks=10)
    tr = Tracker(SPECS)
    tr.update(_view(0, [_ranged_contact()]))
    # Fresh fix at tick 0; query at tick 5 -> gap 5 < threshold 10 -> passive.
    assert pol.choose(_view(5, []), tr) == "passive"


def test_ping_when_stale_active_when_old():
    pol = PingWhenStale(stale_threshold_ticks=10)
    tr = Tracker(SPECS, staleness_ticks=100)
    tr.update(_view(0, [_ranged_contact()]))
    # No new contacts for 15 ticks > threshold -> active.
    for t in range(1, 16):
        tr.update(_view(t, []))
    assert pol.choose(_view(15, []), tr) == "active"
