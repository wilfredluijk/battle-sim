"""Regressions from the September repository review."""
import asyncio
import copy
import math
from pathlib import Path

import pytest
from websockets.exceptions import ConnectionClosed
from websockets.frames import Close

from naval_sdk import Bot, GameStart, Welcome, run_async
from naval_sdk.protocol import Contact, SelfState, WorldView
from naval_sdk.tactical import Tracker
from .test_multiround import FakeWebSocket, WELCOME_FRAME, GAME_START_FRAME, TICK_FRAME


def test_non_object_json_is_ignored(monkeypatch):
    fake = FakeWebSocket([[], None, 42, "hello", WELCOME_FRAME, TICK_FRAME])
    monkeypatch.setattr("naval_sdk.bot.websockets.connect", lambda _: fake)
    bot = Bot()
    assert asyncio.run(run_async(bot)) is None
    assert bot.last_tick == 1
    assert fake.sent[-1]["type"] == "command"


def test_close_during_send_returns_and_clears_socket(monkeypatch):
    class ClosingSocket(FakeWebSocket):
        async def send(self, payload):
            if self.sent:
                raise ConnectionClosed(Close(1000, "done"), None)
            await super().send(payload)
    fake = ClosingSocket([WELCOME_FRAME])
    monkeypatch.setattr("naval_sdk.bot.websockets.connect", lambda _: fake)
    bot = Bot()
    assert asyncio.run(run_async(bot)) is None
    assert bot._ws is None


def test_game_start_refreshes_specs_before_callbacks(monkeypatch):
    start = copy.deepcopy(GAME_START_FRAME)
    start["ship_specs"] = {**WELCOME_FRAME["ship_specs"], "shell_speed": 123.0}
    start["simulation_dt"] = 0.1
    parsed = GameStart.from_dict(start)
    assert parsed.ship_specs.shell_speed == 123.0
    assert parsed.simulation_dt == 0.1
    fake = FakeWebSocket([WELCOME_FRAME, start])
    monkeypatch.setattr("naval_sdk.bot.websockets.connect", lambda _: fake)
    calls = []
    class ConfigBot(Bot):
        def on_welcome(self, welcome):
            calls.append(("specs", welcome.ship_specs.shell_speed))
        def on_game_start(self, *args):
            calls.append(("start", self.welcome.ship_specs.shell_speed))
    asyncio.run(run_async(ConfigBot()))
    assert calls == [("specs", 70.0), ("specs", 123.0), ("start", 123.0)]
    assert [m["type"] for m in fake.sent] == ["hello", "ready"]


def view(tick, speed):
    pos = (100.0 + speed * tick * 0.1, 0.0)
    me = SelfState((0, 0), 0, 0, 100, 250, 0, 0)
    return WorldView(tick, 80, me, [Contact("c_0", "ship", pos, 90, math.hypot(*pos), 1)], [])


@pytest.mark.parametrize("tick_hz", [5, 10, 20, 60])
def test_tracking_uses_simulation_seconds(tick_hz):
    welcome = Welcome.from_dict(WELCOME_FRAME)
    tracker = Tracker(welcome.ship_specs, tick_hz=tick_hz, simulation_dt=welcome.simulation_dt)
    for tick in range(1, 100):
        tracker.update(view(tick, 9))
    assert tracker.tracks[0].vel == pytest.approx((9, 0))


def test_powerful_bot_velocity_uses_observations(monkeypatch):
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[2] / "examples"))
    from powerful_bot import PowerfulBot
    bot = PowerfulBot()
    # Include missed observations as well as consecutive ones.
    for tick in range(1, 200):
        if tick % 7:
            bot._update_tracks(view(tick, 8))
    assert next(iter(bot.state.tracks.values())).vel == pytest.approx((8, 0))
