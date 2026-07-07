"""Connection and message loop for naval-sdk bots.

The trust model: the server is authoritative. The SDK never crashes on a
malformed server message — it logs and continues, because bot authors will hit
edge cases we didn't anticipate.
"""

from __future__ import annotations

import asyncio
import json
import logging
from typing import Any, Dict, List, Optional

try:
    import websockets
    from websockets.exceptions import ConnectionClosed
except ImportError as exc:  # pragma: no cover - import-time error
    raise ImportError(
        "naval_sdk requires the `websockets` package. Install with `pip install websockets`."
    ) from exc

from .protocol import Command, GameOver, Welcome, WorldView

log = logging.getLogger("naval_sdk")


class Bot:
    """Base class for naval-battle bots.

    Subclass this and override `on_tick`. Optionally override `on_welcome`,
    `on_game_start`, `on_game_over`, and `on_error` for richer behavior.
    The reference implementation defaults each callback to a no-op except
    `on_tick`, which must be implemented or the bot will simply hold station.
    """

    # ---- Public state populated by the runtime ----
    welcome: Optional[Welcome] = None
    last_tick: int = 0

    # ---- Callbacks subclasses override ----
    def on_welcome(self, welcome: Welcome) -> None:
        """Fires once, right after the server's `welcome` frame is parsed.

        Use it to stash gameplay constants (`welcome.ship_specs.shell_speed`,
        `max_shell_range`, etc.) on `self` so `on_tick` can read them cheaply.
        Runs before the SDK sends `ready` to the server.
        """

    def choose_powerups(self, welcome: Welcome) -> List[str]:
        """Pick up to two distinct powerups for the match.

        Override to return a list like ``["overdrive", "rapid_fire"]``. The SDK sends
        `select_powerups` to the server before `ready`. Default returns an empty list,
        i.e. play vanilla. The available ids live on `welcome.available_powerups` —
        see ``docs/POWERUPS.md`` for what each one does.

        The server enforces: exactly 2 picks, distinct, all known. An invalid loadout
        earns an `error` frame which the SDK forwards to `on_error`.
        """
        return []

    def on_game_start(self, tick: int, starting_position, starting_heading_deg: float) -> None:
        """Fires when the operator transitions the room to `running`.

        `tick` is the tick at which the match starts (usually 0). The starting
        position and heading are also reflected on the *next* `on_tick`'s
        `view.me`, so most bots can ignore this hook entirely.
        """

    def on_tick(self, view: WorldView) -> Command:
        """Decide what to do this tick. **Override me.**

        Called every simulation tick (default: 10 Hz). Return a `Command` —
        the SDK serializes it back to the server before `view.deadline_ms`
        elapses. If you return `None` or raise, the SDK logs the error and
        emits a hold-station command, keeping the connection alive.

        See README's "Example bots" section for typical patterns.
        """
        return Command(throttle=0.0, rudder=0.0, sensor_mode="active")

    def on_game_over(self, result: GameOver) -> Optional[bool]:
        """Fires when the server announces a match result.

        `result.winner` is the winning `bot_id`, or `None` for a draw / abort.
        The replay JSONL is at `replays/<result.replay_id>.jsonl` on the server.

        Return ``False`` to close the connection and exit the run loop. Return
        ``True`` (or ``None``) to stay connected and wait for the next match —
        the server will eventually emit a ``lobby`` message followed by
        another ``game_start``. The SDK auto-sends ``ready`` again when it
        sees the ``lobby`` frame, so the default behaviour is "stay around
        for the next round".
        """
        return True

    def on_lobby(self, tick: int) -> None:
        """Fires when the server returns the room to the lobby after a match.

        ``tick`` is always 0; it's the next match's starting tick. The SDK
        auto-sends ``ready`` immediately after this callback, so most bots
        can ignore it. Override to reset per-game state (counters, plans).
        """

    def on_error(self, code: str, message: str) -> None:
        """Fires whenever the server sends a typed `error` frame.

        Common codes: `late_command`, `cooldown_active`, `no_ammo`. Override
        to react (e.g. drop sensor pings when you keep missing the deadline).
        Default behaviour is to log at WARNING level.
        """
        log.warning("server error code=%s: %s", code, message)

    # ---- Escape hatches for power users ----
    async def raw_send(self, payload: Dict[str, Any]) -> None:
        """Send an arbitrary JSON object to the server. Bypasses typed API."""
        ws = self._ws
        if ws is None:
            raise RuntimeError("raw_send called before connection is open")
        await ws.send(json.dumps(payload))

    async def raw_recv(self) -> Dict[str, Any]:
        """Block until the next JSON object arrives from the server."""
        ws = self._ws
        if ws is None:
            raise RuntimeError("raw_recv called before connection is open")
        while True:
            frame = await ws.recv()
            if isinstance(frame, bytes):
                log.warning("ignoring unexpected binary frame from server")
                continue
            try:
                return json.loads(frame)
            except json.JSONDecodeError:
                log.warning("ignoring non-JSON frame: %r", frame[:200])

    # ---- Internal ----
    _ws: Optional[Any] = None  # set during a run

    def __init__(self) -> None:
        self.welcome = None
        self.last_tick = 0


async def run_async(
    bot: Bot,
    *,
    host: str = "localhost",
    port: int = 7878,
    name: str = "bot",
    version: str = "naval-sdk/0.1.0",
    path: str = "/bot",
) -> Optional[GameOver]:
    """Connect `bot` to a running server and pump messages until `game_over`.

    Returns the `GameOver` payload, or `None` if the connection closed without one.
    """
    uri = f"ws://{host}:{port}{path}"
    log.info("connecting to %s as %r", uri, name)

    async with websockets.connect(uri) as ws:
        bot._ws = ws

        async def send_loadout_and_ready() -> None:
            """Commit the powerup loadout (if any), then send `ready`.

            The server scopes committed loadouts and `ready` flags per match and
            drops them whenever the room returns to lobby, so this whole sequence
            must be re-sent every time we (re-)enter a lobby — not just once at
            `welcome`. `choose_powerups` needs the `Welcome`; reuse the stored one.
            """
            if bot.welcome is not None:
                picks = _safe_callback_returning(bot.choose_powerups, bot.welcome)
                if picks:
                    await ws.send(
                        json.dumps(
                            {
                                "type": "select_powerups",
                                "powerups": [str(p) for p in picks],
                            }
                        )
                    )
            await ws.send(json.dumps({"type": "ready"}))

        try:
            await ws.send(json.dumps({"type": "hello", "name": name, "version": version}))

            ready_sent = False
            result: Optional[GameOver] = None

            while True:
                try:
                    frame = await ws.recv()
                except ConnectionClosed:
                    log.info("server closed connection")
                    break

                if isinstance(frame, bytes):
                    log.warning("ignoring binary frame from server")
                    continue

                try:
                    msg: Dict[str, Any] = json.loads(frame)
                except json.JSONDecodeError:
                    log.warning("ignoring non-JSON frame: %r", frame[:200])
                    continue

                msg_type = msg.get("type")
                if msg_type == "welcome":
                    try:
                        welcome = Welcome.from_dict(msg)
                    except (KeyError, IndexError, TypeError, ValueError) as exc:
                        log.warning("malformed welcome: %s (frame=%r)", exc, msg)
                        continue
                    bot.welcome = welcome
                    _safe_callback(bot.on_welcome, welcome)
                    if not ready_sent:
                        await send_loadout_and_ready()
                        ready_sent = True

                elif msg_type == "game_start":
                    try:
                        gs_tick = int(msg["tick"])
                        pos = msg["starting_position"]
                        # Convert the position inside the `try`: indexing/converting
                        # at the callback call site would escape this guard and let a
                        # malformed frame (e.g. `"starting_position": 42`) crash the
                        # run loop.
                        start_pos = (float(pos[0]), float(pos[1]))
                        heading = float(msg["starting_heading_deg"])
                    except (KeyError, IndexError, TypeError, ValueError) as exc:
                        log.warning("malformed game_start: %s (frame=%r)", exc, msg)
                        continue
                    _safe_callback(
                        bot.on_game_start,
                        gs_tick,
                        start_pos,
                        heading,
                    )

                elif msg_type == "tick":
                    try:
                        view = WorldView.from_dict(msg)
                    except (KeyError, IndexError, TypeError, ValueError) as exc:
                        log.warning("malformed tick: %s (frame=%r)", exc, msg)
                        continue
                    bot.last_tick = view.tick
                    try:
                        cmd = bot.on_tick(view)
                    except Exception:
                        log.exception("on_tick raised; sending hold-station command")
                        cmd = Command()
                    if cmd is None:
                        cmd = Command()
                    await ws.send(json.dumps(cmd.to_dict(view.tick)))

                elif msg_type == "game_over":
                    try:
                        result = GameOver.from_dict(msg)
                    except (KeyError, IndexError, TypeError, ValueError) as exc:
                        log.warning("malformed game_over: %s (frame=%r)", exc, msg)
                        break
                    keep_running = _safe_callback_returning(bot.on_game_over, result)
                    if keep_running is False:
                        # Bot opted out — close the connection cleanly.
                        break
                    # Otherwise stay connected and wait for the server's `lobby`
                    # frame, which triggers a fresh `ready` send (see below).
                    # Reset the per-match handshake flag so the next welcome (if any)
                    # would re-send ready, though typically only `lobby` arrives.
                    ready_sent = False

                elif msg_type == "lobby":
                    try:
                        lobby_tick = int(msg.get("tick", 0))
                    except (TypeError, ValueError) as exc:
                        log.warning("malformed lobby: %s (frame=%r)", exc, msg)
                        continue
                    _safe_callback(bot.on_lobby, lobby_tick)
                    if not ready_sent:
                        # Re-commit the loadout: the server drops committed
                        # powerups on every return to lobby, so re-sending `ready`
                        # alone would play match 2+ vanilla. The `ready_sent` guard
                        # keeps a repeated `lobby` frame from double-sending.
                        await send_loadout_and_ready()
                        ready_sent = True

                elif msg_type == "error":
                    _safe_callback(
                        bot.on_error,
                        str(msg.get("code", "unknown")),
                        str(msg.get("message", "")),
                    )

                else:
                    log.debug("ignoring unknown message type %r", msg_type)

            return result
        finally:
            bot._ws = None


def run(
    bot: Bot,
    *,
    host: str = "localhost",
    port: int = 7878,
    name: str = "bot",
    version: str = "naval-sdk/0.1.0",
    path: str = "/bot",
) -> Optional[GameOver]:
    """Synchronous wrapper around `run_async` for the common `if __name__ == "__main__"` path."""
    return asyncio.run(
        run_async(bot, host=host, port=port, name=name, version=version, path=path)
    )


def _safe_callback(fn, *args) -> None:
    try:
        fn(*args)
    except Exception:
        log.exception("bot callback %s raised", getattr(fn, "__name__", fn))


def _safe_callback_returning(fn, *args):
    """Like `_safe_callback` but returns the callback's value (or `None` on error)."""
    try:
        return fn(*args)
    except Exception:
        log.exception("bot callback %s raised", getattr(fn, "__name__", fn))
        return None
