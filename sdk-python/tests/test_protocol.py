import math

import pytest

from naval_sdk.protocol import (
    Command,
    FireCommand,
    GameOver,
    HitEvent,
    PowerupActivatedEvent,
    PowerupStatus,
    ShellSplashEvent,
    Welcome,
    WorldView,
)


WELCOME_FRAME = {
    "type": "welcome",
    "bot_id": "b_1",
    "ship_id": "s_1",
    "map": {"width": 700, "height": 700},
    "tick_hz": 10,
    "ship_specs": {
        "max_forward_speed": 9.0,
        "max_reverse_speed": 2.0,
        "acceleration": 3.5,
        "turn_rate_deg_per_s": 20.0,
        "hull_hp": 100,
        "max_ammo": 250,
        "gun_cooldown_ticks": 15,
        "hit_radius": 8.0,
        "shell_speed": 70.0,
        "max_shell_range": 300.0,
        "splash_radius": 15.0,
        "max_splash_damage": 25,
    },
}


TICK_FRAME = {
    "type": "tick",
    "tick": 142,
    "deadline_ms": 80,
    "self": {
        "pos": [200.0, 500.0],
        "heading_deg": 90.0,
        "speed": 4.1,
        "hp": 78,
        "ammo": 14,
        "rudder": -0.3,
        "throttle": 0.8,
    },
    "contacts": [
        {
            "id": "c_a1",
            "kind": "ship",
            "pos": [450.0, 510.0],
            "bearing_deg": 88.0,
            "range": 247.0,
            "confidence": 0.85,
        },
        {
            "id": "c_a2",
            "kind": "ship",
            "pos": [300.0, 500.0],
            "bearing_deg": 90.0,
            "confidence": 0.4,
        },
    ],
    "events": [
        {"type": "hit", "amount": 12},
        {"type": "shell_splash", "pos": [220.0, 505.0]},
    ],
}


def test_welcome_parses():
    w = Welcome.from_dict(WELCOME_FRAME)
    assert w.bot_id == "b_1"
    assert w.ship_specs.shell_speed == pytest.approx(70.0)
    assert w.map.width == 700


def test_world_view_parses():
    view = WorldView.from_dict(TICK_FRAME)
    assert view.tick == 142
    assert view.me.hp == 78
    assert len(view.contacts) == 2
    # passive-style contact has no range
    assert view.contacts[1].range is None
    # events parsed into typed dataclasses
    assert isinstance(view.events[0], HitEvent)
    assert view.events[0].amount == 12
    assert isinstance(view.events[1], ShellSplashEvent)
    assert view.events[1].pos == (220.0, 505.0)


def test_world_view_nearest_contact():
    view = WorldView.from_dict(TICK_FRAME)
    nearest = view.nearest_contact()
    assert nearest is not None
    assert nearest.id == "c_a1"  # only one with range


def test_command_serializes():
    cmd = Command(throttle=1.0, rudder=-0.5, sensor_mode="passive")
    out = cmd.to_dict(tick=10)
    assert out == {
        "type": "command",
        "tick": 10,
        "throttle": 1.0,
        "rudder": -0.5,
        "sensor_mode": "passive",
    }


def test_command_with_fire():
    cmd = Command(throttle=0.0, rudder=0.0, fire=FireCommand(bearing_deg=45.0, range=200.0))
    out = cmd.to_dict(tick=5)
    assert out["fire"] == {"bearing_deg": 45.0, "range": 200.0}


def test_command_fire_at_stationary():
    # Shoot from (100, 100) at a stationary target due east (200, 100).
    cmd = Command().fire_at((200.0, 100.0), shooter_pos=(100.0, 100.0), lead=False)
    assert cmd.fire is not None
    assert cmd.fire.bearing_deg == pytest.approx(90.0)
    assert cmd.fire.range == pytest.approx(100.0)


def test_command_fire_at_leads_moving_target():
    # Target at (100, 0) moving in +y at 10; should aim ahead.
    cmd = Command().fire_at(
        (100.0, 0.0),
        shooter_pos=(0.0, 0.0),
        target_vel=(0.0, 10.0),
        shell_speed=50.0,
        lead=True,
    )
    assert cmd.fire is not None
    # Aim point is north of due-east, so bearing is between 90° and 180°.
    assert 90.0 < cmd.fire.bearing_deg < 180.0


def test_game_over_parses():
    g = GameOver.from_dict({"winner": "b_1", "final_tick": 200, "replay_id": "match_x"})
    assert g.winner == "b_1"
    g2 = GameOver.from_dict({"winner": None, "final_tick": 3000, "replay_id": "draw"})
    assert g2.winner is None


# --- Powerups -----------------------------------------------------------------


def test_welcome_parses_available_powerups():
    frame = dict(WELCOME_FRAME)
    frame["available_powerups"] = ["overdrive", "rapid_fire", "heavy_shell"]
    w = Welcome.from_dict(frame)
    assert "overdrive" in w.available_powerups
    assert len(w.available_powerups) == 3


def test_welcome_default_powerups_is_empty():
    # Backward compatibility: no `available_powerups` key → empty list.
    w = Welcome.from_dict(WELCOME_FRAME)
    assert w.available_powerups == []


def test_self_state_parses_powerup_status_and_convenience_methods_work():
    frame = dict(TICK_FRAME)
    frame["self"] = dict(frame["self"])
    frame["self"]["selected_powerups"] = ["overdrive", "rapid_fire"]
    frame["self"]["powerup_status"] = [
        {"id": "overdrive", "used": False, "active_ticks_left": 0},
        {"id": "rapid_fire", "used": True, "active_ticks_left": 12},
    ]
    view = WorldView.from_dict(frame)
    assert view.me.selected_powerups == ("overdrive", "rapid_fire")
    assert len(view.me.powerup_status) == 2
    assert isinstance(view.me.powerup_status[0], PowerupStatus)
    # Convenience accessors.
    assert view.me.powerup_ready("overdrive")
    assert not view.me.powerup_ready("rapid_fire")  # already used
    assert view.me.powerup_active("rapid_fire")
    assert not view.me.powerup_active("overdrive")
    assert view.me.powerup("rapid_fire").active_ticks_left == 12
    assert view.me.powerup("missing_id") is None


def test_powerup_activated_event_parses():
    frame = dict(TICK_FRAME)
    frame["events"] = [
        {"type": "powerup_activated", "ship_id": "s_2", "powerup": "smoke_screen"}
    ]
    view = WorldView.from_dict(frame)
    assert len(view.events) == 1
    assert isinstance(view.events[0], PowerupActivatedEvent)
    assert view.events[0].ship_id == "s_2"
    assert view.events[0].powerup == "smoke_screen"


def test_command_serializes_activate_powerup():
    cmd = Command(throttle=0.5, activate_powerup="overdrive")
    out = cmd.to_dict(tick=10)
    assert out["activate_powerup"] == "overdrive"


def test_command_without_activation_omits_field():
    # Forward-compatibility: bots that never activate produce the same JSON shape as before.
    cmd = Command(throttle=0.5)
    out = cmd.to_dict(tick=10)
    assert "activate_powerup" not in out
