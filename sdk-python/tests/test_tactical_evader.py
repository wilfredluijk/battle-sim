"""Tests for ``naval_sdk.tactical.evader.Evader``."""

from __future__ import annotations

from naval_sdk.protocol import HitEvent, SelfState, WorldView
from naval_sdk.tactical import Evader, EvaderState


def _view(tick: int, hit: bool = False) -> WorldView:
    me = SelfState(pos=(0.0, 0.0), heading_deg=0.0, speed=0.0, hp=100, ammo=20,
                   rudder=0.0, throttle=0.0)
    events = [HitEvent(amount=10)] if hit else []
    return WorldView(tick=tick, deadline_ms=80, self_state=me, contacts=[], events=events)


def test_evader_idle_returns_none():
    e = Evader()
    assert e.update(_view(0)) is None
    assert e.state == EvaderState.IDLE


def test_evader_triggers_on_hit_then_cools_down():
    e = Evader(evasion_ticks=5, cooldown_ticks=3)
    cmd = e.update(_view(0, hit=True))
    assert cmd is not None
    assert e.state == EvaderState.EVADING
    # All ticks within evasion window return an override.
    for t in range(1, 5):
        assert e.update(_view(t)) is not None
    # Tick 5 transitions to cooldown.
    assert e.update(_view(5)) is None
    assert e.state == EvaderState.COOLDOWN
    # Tick 8 ends cooldown.
    assert e.update(_view(8)) is None
    assert e.state == EvaderState.IDLE


def test_evader_flips_rudder_when_hit_in_cooldown():
    e = Evader(evasion_ticks=3, cooldown_ticks=5)
    e.update(_view(0, hit=True))
    first_cmd = e.update(_view(1))
    initial_sign = first_cmd.rudder

    # Wait out evasion; tick 3 -> cooldown.
    e.update(_view(3))
    assert e.state == EvaderState.COOLDOWN

    # Hit during cooldown -> re-enter evasion with flipped rudder.
    cmd = e.update(_view(4, hit=True))
    assert e.state == EvaderState.EVADING
    assert cmd is not None
    assert cmd.rudder == -initial_sign


def test_evader_override_command_has_full_throttle():
    e = Evader(throttle=1.0)
    cmd = e.update(_view(0, hit=True))
    assert cmd is not None
    assert cmd.throttle == 1.0
    assert abs(cmd.rudder) == 1.0
