# naval-sdk (Python)

Reference Python SDK for the **battle-sim** naval hackathon game. Subclass
`Bot`, override `on_tick`, and the SDK handles the WebSocket transport,
protocol framing, handshake, and message dispatch for you. You write
strategy, not plumbing.

```python
from naval_sdk import Bot, Command, WorldView, run


class Forward(Bot):
    def on_tick(self, view: WorldView) -> Command:
        return Command(throttle=1.0, sensor_mode="active")


if __name__ == "__main__":
    run(Forward(), host="localhost", port=7878, name="forward")
```

That bot connects, completes the handshake, and drives its ship straight
ahead until the match ends.

---

## Table of contents

1. [Install](#install)
2. [Quickstart](#quickstart)
3. [How a match flows](#how-a-match-flows)
4. [API reference](#api-reference)
5. [Coordinates, bearings, and units](#coordinates-bearings-and-units)
6. [Example bots](#example-bots)
7. [Logging and debugging](#logging-and-debugging)
8. [Escape hatches: raw frames](#escape-hatches-raw-frames)
9. [Testing your bot](#testing-your-bot)
10. [Common pitfalls](#common-pitfalls)
11. [Versioning and compatibility](#versioning-and-compatibility)

---

## Install

Requires Python 3.9 or newer. From the repo root:

```bash
cd sdk-python
pip install -e .
```

`-e .` installs in editable mode so changes you make to `naval_sdk/` are
picked up immediately. The only runtime dependency is
[`websockets`](https://pypi.org/project/websockets/). Tests need
`pytest`:

```bash
pip install -e ".[dev]"
pytest                       # runs the SDK's own unit tests
```

---

## Quickstart

```bash
# 1. start the server
cd server
cargo run -- --port 7878 --tick-hz 10 --seed 42

# 2. in another terminal, run your bot
python my_bot.py --host localhost --port 7878 --name kirk

# 3. in the server terminal, start the match
room start main
```

A skeleton `my_bot.py` you can save and run as-is:

```python
"""Drive forward at full throttle, ping with active radar."""

import argparse
import logging

from naval_sdk import Bot, Command, WorldView, run


class Forward(Bot):
    def on_tick(self, view: WorldView) -> Command:
        return Command(throttle=1.0, rudder=0.0, sensor_mode="active")


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser()
    p.add_argument("--host", default="localhost")
    p.add_argument("--port", type=int, default=7878)
    p.add_argument("--name", default="forward")
    args = p.parse_args()
    run(Forward(), host=args.host, port=args.port, name=args.name)
```

---

## How a match flows

Every bot connection follows the same sequence. The SDK drives all of
this for you — the table below is for understanding *what your callbacks
see and when*.

| # | Direction | Frame        | SDK behaviour                                                              |
|---|-----------|--------------|----------------------------------------------------------------------------|
| 1 | bot → srv | `hello`      | Sent automatically when `run()` opens the WebSocket.                       |
| 2 | srv → bot | `welcome`    | SDK parses, stores `bot.welcome`, calls `on_welcome(welcome)`, sends `ready`. |
| 3 | srv → bot | `game_start` | SDK calls `on_game_start(tick, pos, heading_deg)`.                         |
| 4 | srv → bot | `tick` …     | SDK calls `on_tick(view)` and sends your returned `Command` back.          |
| 5 | srv → bot | `game_over`  | SDK calls `on_game_over(result)` once, then closes the connection.         |

Between (2) and (3) the server is in **lobby**: it waits for *all*
connected bots to be ready before starting. Your bot can connect any
time and will simply idle until `game_start` fires.

The server is authoritative on every aspect of the simulation. Your
`Command` is a *request* — throttle and rudder get clamped to `[-1, 1]`,
fire requests get rejected with an `error` frame if the gun is on
cooldown or out of ammo, and command frames that arrive after
`deadline_ms` are dropped (your previous controls persist).

---

## API reference

Everything below is exported from `naval_sdk` and importable as
`from naval_sdk import …`.

### `Bot`

Abstract base class for your bot. Override callbacks you care about.

```python
class Bot:
    welcome: Optional[Welcome]      # set after handshake completes
    last_tick: int                  # tick number of the most recent on_tick

    def on_welcome(self, welcome: Welcome) -> None: ...
    def on_game_start(self,
                      tick: int,
                      starting_position: tuple[float, float],
                      starting_heading_deg: float) -> None: ...
    def on_tick(self, view: WorldView) -> Command: ...        # primary
    def on_game_over(self, result: GameOver) -> None: ...
    def on_error(self, code: str, message: str) -> None: ...  # default: log

    # Escape hatches
    async def raw_send(self, payload: dict) -> None: ...
    async def raw_recv(self) -> dict: ...
```

**`on_tick(view)` is the only callback you must override.** Return a
`Command`. Return `None` or raise an exception and the SDK falls back to
a hold-station command — the bot stays alive, and the exception is
logged.

### `Command`

A mutable per-tick action. Constructor args:

| field         | type                     | default     | notes                                              |
|---------------|--------------------------|-------------|----------------------------------------------------|
| `throttle`    | `float`                  | `0.0`       | Clamped server-side to `[-1, 1]`.                  |
| `rudder`      | `float`                  | `0.0`       | `-1` hard port, `+1` hard starboard.               |
| `sensor_mode` | `"active"` or `"passive"`| `"active"`  | Active gives range, passive is stealth.            |
| `fire`        | `FireCommand` or `None`  | `None`      | Omit to not fire this tick.                        |

```python
Command(throttle=0.8, rudder=-0.3, sensor_mode="passive")
Command().fire_at(target_pos=enemy.pos, shooter_pos=view.me.pos)
```

#### `Command.fire_at(target_pos, *, shooter_pos=None, target_vel=None, shell_speed=50.0, range=None, lead=True)`

Aim a shell at `target_pos`. If `target_vel` is non-zero and
`lead=True`, the SDK computes the lead intercept point using
`lead_target()` and aims there instead. `range` defaults to the distance
to the aim point (the server clamps to `ship_specs.max_shell_range`).

Always pass `shooter_pos=view.me.pos` — without it, the bearing is
computed from the origin and you'll shoot the wrong way.

### `WorldView`

What `on_tick` receives. Read-only.

| field         | type            | notes                                                         |
|---------------|-----------------|---------------------------------------------------------------|
| `tick`        | `int`           | Monotonic tick number.                                        |
| `deadline_ms` | `int`           | How long the server will wait for your `Command`.             |
| `self_state`  | `SelfState`     | Your ship: `pos`, `heading_deg`, `speed`, `hp`, `ammo`, etc.  |
| `me`          | `SelfState`     | Alias for `self_state` (shorter).                             |
| `contacts`    | `list[Contact]` | Filtered by your current sensor mode.                         |
| `events`      | `list[Event]`   | Things you can perceive: own hits, splashes in sensor range.  |

Convenience: `view.nearest_contact()` returns the closest `Contact`
with a known range, or `None`.

### `Contact`

A blip in your sensor view. **Not** a stable ship ID across ticks —
data association is your job.

| field          | type                          |
|----------------|-------------------------------|
| `id`           | `str`, per-tick only          |
| `kind`         | `"ship" \| "shell" \| "unknown"` |
| `pos`          | `tuple[float, float]`         |
| `bearing_deg`  | `float`, absolute compass     |
| `range`        | `Optional[float]`             |
| `confidence`   | `float`                       |

Passive contacts have `range=None` (bearing-only).

### `SelfState`

Your ship's state this tick:
`pos`, `heading_deg`, `speed`, `hp`, `ammo`, `rudder`, `throttle`.

### `ShipSpecs`

Static gameplay constants from `welcome`. Persist on `bot.welcome.ship_specs`.

Most useful fields: `shell_speed` (50.0), `max_shell_range` (300.0),
`gun_cooldown_ticks` (15), `hull_hp` (100), `max_ammo` (20).

### `GameOver`

Final message:

```python
GameOver(winner: Optional[str], final_tick: int, replay_id: str)
```

`winner=None` is a draw.

### Math helpers

All angles in degrees, compass bearings: `0° = north (-y), 90° = east (+x)`.

```python
bearing_to(from_pos, to_pos) -> float            # in [0, 360)
distance(a, b) -> float                          # Euclidean
lead_target(shooter_pos, target_pos, target_vel,
            shell_speed) -> Optional[(x, y)]     # None if unreachable
```

### `run(bot, *, host="localhost", port=7878, name="bot",
       version="naval-sdk/0.1.0", path="/bot") -> Optional[GameOver]`

Synchronous entry point. Wraps `asyncio.run()`. Returns the `GameOver`
payload if the match completed, else `None`.

### `run_async(...) -> Optional[GameOver]`

The same thing, but as a coroutine. Use it if your bot already runs
inside an async program.

---

## Coordinates, bearings, and units

- World coordinates: origin top-left, **+x right**, **+y down** (canvas
  convention).
- Bearings: **0° points along -y** (up on screen), **90° along +x**
  (right). Increase clockwise. Range `[0, 360)`.
- Speeds, distances, and headings are `float`. HP and ammo are `int`.
- Tick rate is set by the server (default `--tick-hz 10`, so
  `dt = 0.1s`).

Because the server's bearing convention is non-trivial, **use
`bearing_to(from_pos, to_pos)` rather than hand-rolling `atan2`**. The
helper returns the value the server expects.

---

## Example bots

### Drift in a circle, fire blind

```python
from naval_sdk import Bot, Command, WorldView, FireCommand, run


class CircleBot(Bot):
    def on_tick(self, view: WorldView) -> Command:
        cmd = Command(throttle=0.6, rudder=0.4, sensor_mode="active")
        if view.tick % 30 == 0 and view.me.ammo > 0:
            cmd.fire = FireCommand(bearing_deg=(view.tick * 11) % 360,
                                   range=250.0)
        return cmd


if __name__ == "__main__":
    run(CircleBot(), name="circle")
```

### Chaser: active radar, pursue the nearest contact

```python
from naval_sdk import Bot, Command, WorldView, bearing_to, run


class ChaserBot(Bot):
    def on_tick(self, view: WorldView) -> Command:
        target = view.nearest_contact()
        if target is None:
            return Command(throttle=0.5, rudder=0.0, sensor_mode="active")

        # Turn toward the target by comparing bearings.
        my_heading = view.me.heading_deg
        want = bearing_to(view.me.pos, target.pos)
        delta = ((want - my_heading + 540) % 360) - 180  # signed in [-180, 180]
        rudder = max(-1.0, min(1.0, delta / 30.0))

        cmd = Command(throttle=1.0, rudder=rudder, sensor_mode="active")

        # Fire when roughly aligned and in range.
        if abs(delta) < 5 and target.range is not None and target.range < 280:
            cmd.fire_at(target.pos, shooter_pos=view.me.pos, lead=False)
        return cmd


if __name__ == "__main__":
    run(ChaserBot(), name="chaser")
```

### Sniper: passive listen, ping only to commit a shot

```python
from naval_sdk import Bot, Command, WorldView, run


class SniperBot(Bot):
    def __init__(self) -> None:
        super().__init__()
        self._last_target_pos = None
        self._ping_for = 0

    def on_tick(self, view: WorldView) -> Command:
        contact = view.nearest_contact() or (
            view.contacts[0] if view.contacts else None
        )

        if self._ping_for > 0:
            self._ping_for -= 1
            mode = "active"
        else:
            mode = "passive"

        # Heard something on passive? Light up briefly to get a range fix.
        if contact and contact.range is None and self._ping_for == 0:
            self._ping_for = 3
            mode = "active"

        cmd = Command(throttle=0.4, rudder=0.0, sensor_mode=mode)

        if contact and contact.range is not None and view.me.ammo > 0:
            # No target_vel here — sniper would estimate it from contact history.
            cmd.fire_at(contact.pos, shooter_pos=view.me.pos, lead=False)
            self._last_target_pos = contact.pos
        return cmd


if __name__ == "__main__":
    run(SniperBot(), name="sniper")
```

### Lifecycle hooks: track per-match stats

```python
from naval_sdk import Bot, Command, GameOver, WorldView, Welcome, run


class StatBot(Bot):
    def on_welcome(self, welcome: Welcome) -> None:
        print(f"I am {welcome.bot_id}, ship {welcome.ship_id}")
        print(f"Shells fly at {welcome.ship_specs.shell_speed} units/s, "
              f"max range {welcome.ship_specs.max_shell_range}")

    def on_game_start(self, tick, pos, heading_deg) -> None:
        print(f"Match started at tick {tick}, starting heading {heading_deg:.1f}°")

    def on_tick(self, view: WorldView) -> Command:
        return Command(throttle=0.5, sensor_mode="passive")

    def on_game_over(self, result: GameOver) -> None:
        if result.winner == self.welcome.bot_id:
            print("Victory.")
        elif result.winner is None:
            print(f"Draw at tick {result.final_tick}.")
        else:
            print(f"Defeated by {result.winner}. Replay: {result.replay_id}")
```

---

## Logging and debugging

The SDK uses Python's stdlib `logging` under the name `naval_sdk`.
Bump it to `DEBUG` to see every frame:

```python
import logging
logging.basicConfig(level=logging.DEBUG)
logging.getLogger("naval_sdk").setLevel(logging.DEBUG)
```

Useful patterns:

- Print `view.deadline_ms` once after `on_welcome` to know your tick
  budget on the *current* server config.
- Log every `Hit` event so you know when the enemy is finding you:
  ```python
  from naval_sdk.protocol import HitEvent
  for ev in view.events:
      if isinstance(ev, HitEvent):
          logging.info("took %d dmg at tick %d", ev.amount, view.tick)
  ```
- `bot.welcome.ship_specs` carries every gameplay constant. Read them
  from there rather than hard-coding numbers — that way your bot keeps
  working if balance changes.

---

## Escape hatches: raw frames

If the typed API doesn't fit (you're debugging, prototyping a new
protocol field, or building an inspector), bypass it:

```python
class LurkBot(Bot):
    async def on_tick(self, view):           # WARNING: see note below
        await self.raw_send({"type": "command", "tick": view.tick,
                             "throttle": 0.0, "rudder": 0.0,
                             "sensor_mode": "active"})
        return Command()  # ignored by the SDK once you've already sent
```

`raw_send`/`raw_recv` are async because they hit the live socket. They
work inside `on_tick` only if you make your override `async` — the SDK
will await it. Most bots don't need this; the typed `Command` is enough.

---

## Testing your bot

You don't need a running server to unit-test logic. Build a `WorldView`
in-process and call `on_tick` directly:

```python
from naval_sdk.protocol import WorldView

frame = {
    "type": "tick",
    "tick": 100,
    "deadline_ms": 80,
    "self": {"pos": [200, 500], "heading_deg": 90.0, "speed": 4.1,
             "hp": 100, "ammo": 14, "rudder": 0.0, "throttle": 0.8},
    "contacts": [{"id": "c1", "kind": "ship", "pos": [450, 510],
                  "bearing_deg": 88.0, "range": 247.0, "confidence": 0.85}],
    "events": [],
}
view = WorldView.from_dict(frame)
cmd = MyBot().on_tick(view)
assert cmd.fire is not None
assert cmd.fire.bearing_deg == pytest.approx(88.0, abs=2.0)
```

Replay tests on the server side already guarantee determinism — if your
bot uses `random`, seed it yourself so your matches are reproducible.

---

## Common pitfalls

- **Forgetting `shooter_pos`** — `fire_at(target.pos)` without
  `shooter_pos=view.me.pos` computes the bearing from the origin, not
  from your ship. Always pass it.
- **Hand-rolled bearings** — `math.atan2(dy, dx)` gives radians from
  +x. The server wants compass degrees from -y, clockwise. Use
  `bearing_to()`.
- **Passive contacts have no range** — `contact.range` is `Optional`.
  Guard before doing math on it.
- **Active mode is loud** — anyone on the map can see your bearing
  while you're pinging, regardless of distance. Don't camp on
  `"active"` unless you mean to.
- **Stable contact IDs are a myth** — `contact.id` is per-tick. To
  track an enemy across ticks, key on position/bearing similarity
  yourself.
- **Tick deadline is real** — the default is 80 ms. If your `on_tick`
  blocks longer (heavy planning, I/O, sleeps), your command is dropped
  and the previous tick's controls persist.

---

## Versioning and compatibility

- `naval_sdk.__version__` is the SDK's own version. The wire protocol
  version comes from the server in the `welcome` frame.
- Additive server changes (new optional fields, new event types) are
  parsed but ignored by older SDKs — your bot keeps working.
- Breaking server changes bump the version string and are documented in
  `docs/PROTOCOL.md`. Pin the SDK version alongside your bot if you
  care about reproducibility.

See the parallel Java SDK in `sdk-java/` for the JVM equivalent.
