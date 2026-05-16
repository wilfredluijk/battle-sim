"""Tests for the SDK's multi-round lifecycle: bots survive `game_over`, re-ready
on `lobby`, and can opt out by returning `False` from `on_game_over`."""

from __future__ import annotations

import asyncio
import json
from typing import Any, Dict, List, Optional

import pytest

from naval_sdk import bot as bot_module
from naval_sdk.bot import Bot, run_async
from naval_sdk.protocol import Command, GameOver, Welcome, WorldView


WELCOME_FRAME = {
    "type": "welcome",
    "bot_id": "b_1",
    "ship_id": "s_1",
    "map": {"width": 1000, "height": 1000},
    "tick_hz": 10,
    "ship_specs": {
        "max_forward_speed": 6.0,
        "max_reverse_speed": 2.0,
        "acceleration": 1.5,
        "turn_rate_deg_per_s": 15.0,
        "hull_hp": 100,
        "max_ammo": 20,
        "gun_cooldown_ticks": 15,
        "hit_radius": 8.0,
        "shell_speed": 50.0,
        "max_shell_range": 300.0,
        "splash_radius": 15.0,
        "max_splash_damage": 25,
    },
}

GAME_START_FRAME = {
    "type": "game_start",
    "tick": 0,
    "starting_position": [500.0, 500.0],
    "starting_heading_deg": 0.0,
}

TICK_FRAME = {
    "type": "tick",
    "tick": 1,
    "deadline_ms": 80,
    "self": {
        "pos": [500.0, 500.0],
        "heading_deg": 0.0,
        "speed": 0.0,
        "hp": 100,
        "ammo": 20,
        "rudder": 0.0,
        "throttle": 0.0,
    },
    "contacts": [],
    "events": [],
}

GAME_OVER_FRAME = {
    "type": "game_over",
    "winner": "b_2",
    "final_tick": 142,
    "replay_id": "match_test",
}

LOBBY_FRAME = {"type": "lobby", "tick": 0}


class FakeWebSocket:
    """Async fake — feed it a script of inbound frames; recv() drains them in order,
    then raises a `ConnectionClosed` so the SDK's run loop exits cleanly."""

    def __init__(self, inbound: List[Dict[str, Any]]):
        self.inbound = list(inbound)
        self.sent: List[Dict[str, Any]] = []

    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc, tb):
        return False

    async def send(self, payload: str):
        self.sent.append(json.loads(payload))

    async def recv(self) -> str:
        if not self.inbound:
            from websockets.exceptions import ConnectionClosed
            from websockets.frames import Close

            raise ConnectionClosed(Close(1000, "test done"), None)
        msg = self.inbound.pop(0)
        return json.dumps(msg)


@pytest.fixture
def patched_connect(monkeypatch):
    """Replace `websockets.connect` with a factory bound to a script."""
    captured: Dict[str, FakeWebSocket] = {}

    def install(frames: List[Dict[str, Any]]) -> FakeWebSocket:
        fake = FakeWebSocket(frames)
        captured["fake"] = fake

        def connect_factory(_uri: str, **_kwargs: Any) -> FakeWebSocket:
            return fake

        monkeypatch.setattr(bot_module.websockets, "connect", connect_factory)
        return fake

    return install


class RecordingBot(Bot):
    def __init__(self, return_after_game_over: Optional[bool] = True) -> None:
        super().__init__()
        self._game_over_return = return_after_game_over
        self.welcomes: List[Welcome] = []
        self.game_starts: List[int] = []
        self.ticks: List[int] = []
        self.game_overs: List[GameOver] = []
        self.lobbies: List[int] = []

    def on_welcome(self, welcome: Welcome) -> None:
        self.welcomes.append(welcome)

    def on_game_start(self, tick: int, _pos, _heading: float) -> None:
        self.game_starts.append(tick)

    def on_tick(self, view: WorldView) -> Command:
        self.ticks.append(view.tick)
        return Command(throttle=0.0, rudder=0.0, sensor_mode="passive")

    def on_game_over(self, result: GameOver):
        self.game_overs.append(result)
        return self._game_over_return

    def on_lobby(self, tick: int) -> None:
        self.lobbies.append(tick)


def test_two_back_to_back_matches_use_single_connection(patched_connect):
    fake = patched_connect(
        [
            WELCOME_FRAME,
            GAME_START_FRAME,
            TICK_FRAME,
            GAME_OVER_FRAME,
            LOBBY_FRAME,
            GAME_START_FRAME,
            TICK_FRAME,
            GAME_OVER_FRAME,
        ]
    )

    bot = RecordingBot()
    asyncio.run(run_async(bot, host="localhost", port=0))

    assert len(bot.welcomes) == 1, "welcome only fires once per connection"
    assert len(bot.game_starts) == 2, "two matches played"
    assert len(bot.ticks) == 2, "one tick per match"
    assert len(bot.game_overs) == 2, "two game_overs"
    assert len(bot.lobbies) == 1, "one lobby between matches"

    # Sent frames: hello, ready, command (round1), ready (after lobby), command (round2)
    types = [m.get("type") for m in fake.sent]
    assert types == ["hello", "ready", "command", "ready", "command"]


def test_on_game_over_false_disconnects_immediately(patched_connect):
    patched_connect(
        [
            WELCOME_FRAME,
            GAME_START_FRAME,
            TICK_FRAME,
            GAME_OVER_FRAME,
            LOBBY_FRAME,  # never consumed
            GAME_START_FRAME,  # never consumed
        ]
    )

    bot = RecordingBot(return_after_game_over=False)
    asyncio.run(run_async(bot, host="localhost", port=0))

    assert len(bot.game_overs) == 1
    assert len(bot.lobbies) == 0, "bot opted out, should not see lobby"
    assert len(bot.game_starts) == 1, "second match never reached"


def test_default_on_game_over_keeps_running(patched_connect):
    """A subclass that doesn't override on_game_over keeps the connection alive."""

    fake = patched_connect(
        [
            WELCOME_FRAME,
            GAME_START_FRAME,
            GAME_OVER_FRAME,
            LOBBY_FRAME,
            GAME_START_FRAME,
            GAME_OVER_FRAME,
        ]
    )

    class _Defaults(Bot):
        def __init__(self) -> None:
            super().__init__()
            self.game_overs = 0

        def on_tick(self, view: WorldView) -> Command:
            return Command()

    bot = _Defaults()
    asyncio.run(run_async(bot, host="localhost", port=0))
    # Two `ready` frames means the bot sat through both rounds.
    types = [m.get("type") for m in fake.sent]
    assert types.count("ready") == 2
