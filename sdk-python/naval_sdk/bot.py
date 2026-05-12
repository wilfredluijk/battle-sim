"""Connection and message loop for naval-sdk bots.

The trust model: the server is authoritative. The SDK never crashes on a
malformed server message — it logs and continues, because bot authors will hit
edge cases we didn't anticipate.
"""

from __future__ import annotations

import asyncio
import json
import logging
from typing import Any, Dict, Optional

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
        """Called once, right after the handshake completes."""

    def on_game_start(self, tick: int, starting_position, starting_heading_deg: float) -> None:
        """Called when the room transitions to `running`."""

    def on_tick(self, view: WorldView) -> Command:
        """Called every tick. Return a `Command`; default holds station."""
        return Command(throttle=0.0, rudder=0.0, sensor_mode="active")

    def on_game_over(self, result: GameOver) -> None:
        """Called once, just before the connection closes."""

    def on_error(self, code: str, message: str) -> None:
        """Called whenever the server sends an `error` frame."""
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
                    except (KeyError, TypeError, ValueError) as exc:
                        log.warning("malformed welcome: %s (frame=%r)", exc, msg)
                        continue
                    bot.welcome = welcome
                    _safe_callback(bot.on_welcome, welcome)
                    if not ready_sent:
                        await ws.send(json.dumps({"type": "ready"}))
                        ready_sent = True

                elif msg_type == "game_start":
                    try:
                        gs_tick = int(msg["tick"])
                        pos = msg["starting_position"]
                        heading = float(msg["starting_heading_deg"])
                    except (KeyError, TypeError, ValueError) as exc:
                        log.warning("malformed game_start: %s (frame=%r)", exc, msg)
                        continue
                    _safe_callback(
                        bot.on_game_start,
                        gs_tick,
                        (float(pos[0]), float(pos[1])),
                        heading,
                    )

                elif msg_type == "tick":
                    try:
                        view = WorldView.from_dict(msg)
                    except (KeyError, TypeError, ValueError) as exc:
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
                    except (KeyError, TypeError, ValueError) as exc:
                        log.warning("malformed game_over: %s (frame=%r)", exc, msg)
                        break
                    _safe_callback(bot.on_game_over, result)
                    break

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
