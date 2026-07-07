"""Tests for the SDK's multi-round lifecycle: bots survive `game_over`, re-ready
on `lobby`, and can opt out by returning `False` from `on_game_over`."""

from __future__ import annotations

import asyncio
import json
from typing import Any, Dict, List, Optional

import pytest

from naval_sdk import bot as bot_module
from naval_sdk.bot import Bot, run_async
from naval_sdk.protocol import (
    Command,
    Contact,
    GameOver,
    MapInfo,
    SelfState,
    ShipSpecs,
    Welcome,
    WorldView,
)
from naval_sdk.tactical import Gunner, Intent, TacticalBot, TacticalContext
from naval_sdk.tactical.tracker import Track, Tracker


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


# ---------------------------------------------------------------------------
# Monte-Carlo tick-reset regression tests (immortal "ghost" contact bug).
#
# In MC mode the connection persists across many matches; the server resets
# ``world.tick`` to 0 and sends only ``game_start`` (no fresh ``welcome``). A
# carried-over track then has ``tick - last_seen_tick < 0`` and, before the fix,
# was never pruned — surviving forever as a phantom contact that wedged reactive
# bots into a saturated engage. These tests guard both halves of the fix:
#   (1) the Tracker's staleness prune treats negative elapsed as stale, and
#   (2) ``TacticalBot.on_game_start`` resets per-match state.
# ---------------------------------------------------------------------------

_SPECS = ShipSpecs(
    max_forward_speed=9.0,
    max_reverse_speed=2.0,
    acceleration=3.5,
    turn_rate_deg_per_s=20.0,
    hull_hp=100,
    max_ammo=250,
    gun_cooldown_ticks=15,
    hit_radius=8.0,
    shell_speed=70.0,
    max_shell_range=300.0,
    splash_radius=15.0,
    max_splash_damage=25,
)


def _mc_view(tick: int, contacts, me_pos=(350.0, 350.0)) -> WorldView:
    me = SelfState(
        pos=me_pos,
        heading_deg=0.0,
        speed=0.0,
        hp=100,
        ammo=100,
        rudder=0.0,
        throttle=0.0,
    )
    return WorldView(
        tick=tick, deadline_ms=80, self_state=me, contacts=contacts, events=[]
    )


def _mc_contact(cid, pos) -> Contact:
    rng = (pos[0] ** 2 + pos[1] ** 2) ** 0.5
    return Contact(
        id=cid,
        kind="ship",
        pos=pos,
        bearing_deg=0.0,
        range=rng,
        confidence=1.0,
    )


def _mc_welcome() -> Welcome:
    return Welcome(
        bot_id="b_1",
        ship_id="s_1",
        map=MapInfo(width=700, height=700),
        tick_hz=10,
        ship_specs=_SPECS,
    )


class _EngageBot(TacticalBot):
    """Reactive bot: engage the nearest threat, else hold."""

    def decide(self, ctx: TacticalContext) -> Intent:
        threat = ctx.threats.nearest()
        return Intent.engage(threat) if threat is not None else Intent.hold()


def test_tracker_no_immortal_ghost_after_tick_reset() -> None:
    """A long match followed by a tick-reset short match leaves no ghost track.

    Drives the Tracker through the public ``update(view)`` API: ticks rise to
    ~2998, then a new match runs 0..300. Asserts no track is ever seen "in the
    future" (``last_seen_tick > current tick``) and the live count returns to
    zero (the carried track prunes instead of becoming immortal).
    """
    tracker = Tracker(_SPECS, tick_hz=10, staleness_ticks=40)

    # Long match: a contact present near the end so a track carries into the
    # reset. (Earlier ticks with no contact would just prune normally.)
    for t in range(0, 2999):
        if t >= 2990:
            tracker.update(_mc_view(t, [_mc_contact("e1", (600.0, 500.0))]))
        else:
            tracker.update(_mc_view(t, []))
    assert len(tracker.tracks) >= 1, "expected a live track entering the reset"

    # New match: server resets tick to 0. Even if the bot forgot to reset the
    # tracker, the staleness fix must prevent an immortal ghost.
    for t in range(0, 301):
        tracks = tracker.update(_mc_view(t, []))
        for tr in tracks:
            assert tr.last_seen_tick <= t, (
                f"ghost track {tr.track_id} last_seen={tr.last_seen_tick} > tick={t}"
            )

    assert tracker.tracks == [], "carried-over track must prune after tick reset"


def test_tracker_live_count_recovers_after_reset_match() -> None:
    """After a tick-reset match the live count matches what's actually seen."""
    tracker = Tracker(_SPECS, tick_hz=10, staleness_ticks=40)
    for t in range(0, 2999):
        tracker.update(_mc_view(t, [_mc_contact("e1", (600.0, 500.0))]))
    assert len(tracker.tracks) == 1

    # New match, tick reset, a single fresh contact: exactly one live track,
    # and it's the newly spawned one (no ghost lingering alongside it).
    for t in range(0, 60):
        tracker.update(_mc_view(t, [_mc_contact("e2", (610.0, 500.0))]))
    live = tracker.tracks
    assert len(live) == 1, f"expected exactly one live track, got {len(live)}"
    assert live[0].last_seen_tick <= 59


def test_tracker_reset_clears_all_state() -> None:
    """``Tracker.reset()`` drops tracks/history and rewinds the id counter."""
    tracker = Tracker(_SPECS, tick_hz=10)
    for t in range(5):
        tracker.update(_mc_view(t, [_mc_contact("e1", (600.0, 500.0))]))
    assert len(tracker.tracks) >= 1

    tracker.reset()
    assert tracker.tracks == []

    # Fresh spawn after reset starts ids at 1 again (no carried counter).
    tracker.update(_mc_view(0, [_mc_contact("e1", (600.0, 500.0))]))
    assert tracker.tracks[0].track_id == 1


def test_tacticalbot_game_start_clears_tracker_immediately() -> None:
    """``TacticalBot.on_game_start`` must clear the tracker right away.

    Regression guard for the immortal-ghost bug: a bot reused across matches
    must not enter the next match with the previous match's contacts loaded.
    """
    bot = _EngageBot()
    bot.on_welcome(_mc_welcome())
    for t in range(5):
        bot.on_tick(_mc_view(t, [_mc_contact("e1", (600.0, 500.0))]))
    assert bot.tracker is not None and len(bot.tracker.tracks) >= 1

    bot.on_game_start(0, (500.0, 500.0), 0.0)
    assert bot.tracker.tracks == [], "tracker must be empty immediately after game_start"
    assert bot._patrol_corner == 0
    assert bot.evader is not None and bot.evader.state.value == "idle"
    # Welcome-derived config must survive the reset.
    assert bot.welcome is not None and bot.helm is not None and bot.gunner is not None


def test_back_to_back_matches_not_pinned_to_saturated_command() -> None:
    """Across matches with a tick reset, commands must not pin to one saturated value.

    The original bug saturated reactive bots (throttle=1.0, rudder=±1.0) into
    permanently engaging a phantom carried from a prior match. After the fix, a
    contact-free match must not inherit the previous match's engage behaviour.
    """
    bot = _EngageBot()
    bot.on_welcome(_mc_welcome())

    # Match 1: long engagement against a real contact (high tick counts).
    for t in range(0, 60):
        bot.on_tick(_mc_view(t, [_mc_contact("e1", (600.0, 500.0))]))

    # Match 2: server resets tick to 0, NO contacts at all this match.
    bot.on_game_start(0, (500.0, 500.0), 0.0)

    # Core invariant: per-match state was rebuilt — no ghost track survives, the
    # patrol cursor is rewound, and the Evader is back to IDLE so it cannot
    # preempt the new match with a carried evasive override.
    assert bot.tracker is not None and bot.tracker.tracks == []
    assert bot._patrol_corner == 0
    assert bot.evader is not None and bot.evader.state.value == "idle"

    # Driving the contact-free match: with no tracks, decide() holds station, so
    # no command may be pinned to a fully-saturated engage against a ghost.
    cmds = [bot.on_tick(_mc_view(t, [])) for t in range(0, 30)]
    assert all(c is not None for c in cmds)
    saturated = [
        c for c in cmds if abs(c.throttle) >= 0.999 and abs(c.rudder) >= 0.999
    ]
    assert not saturated, "bot is saturated against a ghost after tick reset"
    assert bot.tracker.tracks == [], "no ghost track may appear in a contact-free match"


# ---------------------------------------------------------------------------
# F-04 — powerup loadout must be re-committed after every lobby.
#
# The server drops committed loadouts on every return to lobby, so a bot that
# only selects powerups once (at welcome) plays match 2+ vanilla. The SDK must
# re-send `select_powerups` before `ready` after each lobby, with the same picks.
# ---------------------------------------------------------------------------


class _PowerupBot(RecordingBot):
    PICKS = ["overdrive", "rapid_fire"]

    def choose_powerups(self, welcome: Welcome) -> List[str]:
        return list(self.PICKS)


def test_powerups_recommitted_after_each_lobby(patched_connect):
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
            LOBBY_FRAME,
            GAME_START_FRAME,
            TICK_FRAME,
            GAME_OVER_FRAME,
        ]
    )

    bot = _PowerupBot()
    asyncio.run(run_async(bot, host="localhost", port=0))

    # Every `ready` must be immediately preceded by a matching `select_powerups`.
    types = [m.get("type") for m in fake.sent]
    ready_indices = [i for i, t in enumerate(types) if t == "ready"]
    assert len(ready_indices) == 3, "one ready per match (welcome + two lobbies)"
    for i in ready_indices:
        assert i >= 1 and types[i - 1] == "select_powerups", (
            "select_powerups must precede every ready"
        )
        assert fake.sent[i - 1]["powerups"] == _PowerupBot.PICKS, (
            "same picks re-sent each lobby"
        )


def test_no_powerups_sends_ready_only(patched_connect):
    """A bot with the default (empty) choose_powerups sends `ready` with no loadout."""
    fake = patched_connect([WELCOME_FRAME, GAME_START_FRAME, TICK_FRAME, GAME_OVER_FRAME])
    bot = RecordingBot()
    asyncio.run(run_async(bot, host="localhost", port=0))
    types = [m.get("type") for m in fake.sent]
    assert "select_powerups" not in types
    assert types == ["hello", "ready", "command"]


# ---------------------------------------------------------------------------
# F-05 — Gunner cooldown must reset across matches (server ticks reset to 0).
# ---------------------------------------------------------------------------


def _active_track(pos=(100.0, 0.0)) -> Track:
    return Track(
        track_id=1,
        kind="ship",
        pos=pos,
        observed_pos=pos,
        vel=(0.0, 0.0),
        last_seen_tick=0,
        first_seen_tick=0,
        last_active_tick=0,
        confidence=1.0,
        source="active",
    )


def test_gunner_reset_clears_stale_cooldown():
    gunner = Gunner(_SPECS)
    track = _active_track()
    view = _mc_view(0, [], me_pos=(0.0, 0.0))

    # Fire late in match 1 → cooldown threshold sits far in the future.
    gunner.note_fired(2990)
    assert gunner.solve(view.me, track, view) is None, (
        "stale absolute-tick cooldown should still gate a shot at tick 0"
    )

    # New match at tick 0: without a reset the gunner refuses to fire all match.
    gunner.reset()
    assert gunner.solve(view.me, track, view) is not None, (
        "after reset a valid track must yield a solution at tick 0"
    )


def test_tacticalbot_game_start_resets_gunner():
    bot = _EngageBot()
    bot.on_welcome(_mc_welcome())
    assert bot.gunner is not None
    bot.gunner.note_fired(2990)
    assert bot.gunner.next_fire_tick == 2990 + _SPECS.gun_cooldown_ticks

    bot.on_game_start(0, (500.0, 500.0), 0.0)
    assert bot.gunner.next_fire_tick == 0, "on_game_start must reset the gunner cooldown"


# ---------------------------------------------------------------------------
# F-06 — malformed `game_start` must not crash the run loop.
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("bad_pos", [42, [], ["x", "y"], None])
def test_malformed_game_start_does_not_crash(patched_connect, bad_pos):
    bad_frame = dict(GAME_START_FRAME)
    if bad_pos is None:
        del bad_frame["starting_position"]
    else:
        bad_frame["starting_position"] = bad_pos

    fake = patched_connect(
        [
            WELCOME_FRAME,
            bad_frame,  # must be logged and skipped, not crash
            GAME_START_FRAME,  # a subsequent valid frame is still processed
            TICK_FRAME,
            GAME_OVER_FRAME,
        ]
    )

    bot = RecordingBot()
    # Must return normally (no exception escaping run_async).
    asyncio.run(run_async(bot, host="localhost", port=0))

    assert bot.game_starts == [0], "only the valid game_start reaches the callback"
    assert bot.ticks == [1], "the loop kept running after the malformed frame"
    types = [m.get("type") for m in fake.sent]
    assert "command" in types
