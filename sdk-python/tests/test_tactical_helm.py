"""Tests for ``naval_sdk.tactical.helm.Helm``."""

from __future__ import annotations

import pytest

from naval_sdk.protocol import SelfState, ShipSpecs
from naval_sdk.tactical import Helm

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


def _me(pos=(400.0, 400.0), heading=0.0):
    return SelfState(pos=pos, heading_deg=heading, speed=4.0, hp=100, ammo=20,
                     rudder=0.0, throttle=0.0)


def test_helm_aligned_runs_full_throttle_zero_rudder():
    h = Helm(SPECS, map_width=800.0, map_height=800.0)
    throttle, rudder = h.steer_to_bearing(_me(heading=90.0), 90.0)
    assert rudder == pytest.approx(0.0, abs=1e-9)
    assert throttle == pytest.approx(1.0)


def test_helm_rudder_sign_matches_required_turn_direction():
    h = Helm(SPECS, turn_aggression_deg=30.0)
    # Heading 0, target bearing 90 -> needs to turn right (positive).
    _, r_right = h.steer_to_bearing(_me(heading=0.0), 90.0)
    assert r_right > 0.0
    # Heading 0, target bearing 270 (= -90) -> needs to turn left.
    _, r_left = h.steer_to_bearing(_me(heading=0.0), 270.0)
    assert r_left < 0.0


def test_helm_tapers_throttle_for_sharp_turns():
    h = Helm(SPECS, min_turn_throttle=0.5)
    throttle_aligned, _ = h.steer_to_bearing(_me(heading=0.0), 0.0)
    throttle_sharp, _ = h.steer_to_bearing(_me(heading=0.0), 180.0)
    assert throttle_sharp < throttle_aligned
    assert throttle_sharp >= 0.5


def test_helm_wall_override_pushes_inward():
    h = Helm(SPECS, map_width=800.0, map_height=800.0, wall_margin=30.0)
    # Hugging the north wall (y small), target bearing pointing north (0°)
    # would drive us into the wall. Helm should redirect to a southerly bearing.
    me = _me(pos=(400.0, 10.0), heading=0.0)
    _, _ = h.steer_to_bearing(me, 0.0)  # ensure no exception
    # The redirected bearing should aim *away* from the wall: southward (~180°).
    redirected = h._wall_override(me, target_bearing=0.0)
    assert 90.0 < redirected < 270.0


def test_helm_wall_override_leaves_target_alone_if_safe():
    h = Helm(SPECS, map_width=800.0, map_height=800.0, wall_margin=30.0)
    me = _me(pos=(400.0, 10.0))
    # Target south (away from north wall) — no override needed.
    redirected = h._wall_override(me, target_bearing=180.0)
    assert redirected == pytest.approx(180.0)
