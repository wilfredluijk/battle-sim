"""End-to-end style test: drive ``TacticalBot`` through a synthetic match.

No server, no network. We construct ``Welcome`` and a sequence of
``WorldView`` frames and assert that the bot produces sensible commands
across the documented preemption priorities (evasion > intent > gunner).
"""

from __future__ import annotations

from typing import List

from naval_sdk.helpers import bearing_to
from naval_sdk.protocol import (
    Contact,
    HitEvent,
    MapInfo,
    SelfState,
    ShipSpecs,
    Welcome,
    WorldView,
)
from naval_sdk.tactical import Intent, TacticalBot, TacticalContext

SPECS = ShipSpecs(
    max_forward_speed=6.0, max_reverse_speed=2.0, acceleration=1.5,
    turn_rate_deg_per_s=15.0, hull_hp=100, max_ammo=20, gun_cooldown_ticks=15,
    hit_radius=2.0, shell_speed=50.0, max_shell_range=300.0,
    splash_radius=15.0, max_splash_damage=25,
)

WELCOME = Welcome(
    bot_id="b1",
    ship_id="s1",
    map=MapInfo(width=800, height=800),
    tick_hz=10,
    ship_specs=SPECS,
)


def _me(pos=(400.0, 400.0), heading=0.0, hp=100, ammo=20):
    return SelfState(pos=pos, heading_deg=heading, speed=0.0, hp=hp, ammo=ammo,
                     rudder=0.0, throttle=0.0)


def _ship_contact(my_pos, enemy_pos):
    rng = ((enemy_pos[0] - my_pos[0]) ** 2 + (enemy_pos[1] - my_pos[1]) ** 2) ** 0.5
    return Contact(id="e", kind="ship", pos=enemy_pos,
                   bearing_deg=bearing_to(my_pos, enemy_pos), range=rng, confidence=1.0)


def _view(tick, me, contacts=None, events=None):
    return WorldView(tick=tick, deadline_ms=80, self_state=me,
                     contacts=contacts or [], events=events or [])


class EngageNearest(TacticalBot):
    def decide(self, ctx: TacticalContext) -> Intent:
        if ctx.threats:
            return Intent.engage(ctx.threats.nearest())
        return Intent.hold()


def test_tacticalbot_holds_with_no_contacts():
    bot = EngageNearest()
    bot.on_welcome(WELCOME)
    cmd = bot.on_tick(_view(0, _me()))
    assert cmd.throttle == 0.0
    assert cmd.rudder == 0.0
    assert cmd.fire is None


def test_tacticalbot_steers_toward_engaged_target():
    bot = EngageNearest()
    bot.on_welcome(WELCOME)
    me_pos = (400.0, 400.0)
    enemy_pos = (600.0, 400.0)  # due east
    me = _me(pos=me_pos, heading=0.0)
    cmd = bot.on_tick(_view(0, me, [_ship_contact(me_pos, enemy_pos)]))
    # Heading is 0 (north); target is east. Should turn right.
    assert cmd.rudder > 0.0
    assert cmd.throttle > 0.0


def test_tacticalbot_fires_when_target_in_range_and_lined_up():
    bot = EngageNearest()
    bot.on_welcome(WELCOME)
    me_pos = (400.0, 400.0)
    enemy_pos = (500.0, 400.0)  # 100 units east — comfortably in range.
    me = _me(pos=me_pos, heading=90.0)
    cmd = bot.on_tick(_view(0, me, [_ship_contact(me_pos, enemy_pos)]))
    assert cmd.fire is not None
    # Cooldown should now block subsequent shots.
    cmd2 = bot.on_tick(_view(1, me, [_ship_contact(me_pos, enemy_pos)]))
    assert cmd2.fire is None


def test_tacticalbot_evasion_preempts_intent():
    bot = EngageNearest()
    bot.on_welcome(WELCOME)
    me_pos = (400.0, 400.0)
    enemy_pos = (500.0, 400.0)
    me = _me(pos=me_pos, heading=90.0)
    cmd = bot.on_tick(
        _view(0, me, [_ship_contact(me_pos, enemy_pos)], events=[HitEvent(amount=10)])
    )
    # Evasion override: hard rudder + full throttle. The exact rudder sign is
    # implementation-defined, but its magnitude must be 1.
    assert abs(cmd.rudder) == 1.0
    assert cmd.throttle == 1.0
    # No fire during evasion — the override Command is a fresh Command(...).
    assert cmd.fire is None


def test_tacticalbot_custom_intent_passes_through():
    from naval_sdk.protocol import Command

    class Custom(TacticalBot):
        def decide(self, ctx):
            return Intent.custom(Command(throttle=-0.5, rudder=0.25, sensor_mode="passive"))

    bot = Custom()
    bot.on_welcome(WELCOME)
    cmd = bot.on_tick(_view(0, _me()))
    assert cmd.throttle == -0.5
    assert cmd.rudder == 0.25
    assert cmd.sensor_mode == "passive"


def test_tacticalbot_full_match_loop_does_not_explode():
    """Run 50 ticks against a moving target. Just assert nothing crashes."""
    bot = EngageNearest()
    bot.on_welcome(WELCOME)

    me_pos = (400.0, 400.0)
    enemy_pos = [600.0, 400.0]
    for tick in range(50):
        enemy_pos[0] -= 1.0  # closing
        me = _me(pos=me_pos, heading=90.0, ammo=20 - tick // 15)
        contacts = [_ship_contact(me_pos, tuple(enemy_pos))]
        cmd = bot.on_tick(_view(tick, me, contacts))
        assert -1.0 <= cmd.throttle <= 1.0
        assert -1.0 <= cmd.rudder <= 1.0
        assert cmd.sensor_mode in ("active", "passive")
