# naval-sdk

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

This document is the single source for bot authors. The companion
[`../docs/PROTOCOL.md`](../docs/PROTOCOL.md) covers the wire protocol
spec (frame shapes, exact field semantics) — you only need it if you're
debugging at the wire level or building a non-Python client.

---

## Table of contents

1. [Install](#install)
2. [Quickstart](#quickstart)
3. [What you're building](#what-youre-building)
4. [The world and its rules](#the-world-and-its-rules)
5. [What you see each tick](#what-you-see-each-tick)
6. [How a match flows](#how-a-match-flows)
7. [Coordinates, bearings, and units](#coordinates-bearings-and-units)
8. [Base API reference](#base-api-reference)
9. [Tactical toolkit](#tactical-toolkit)
10. [Example bots](#example-bots)
11. [Errors and how to react](#errors-and-how-to-react)
12. [Common pitfalls](#common-pitfalls)
13. [Logging and debugging](#logging-and-debugging)
14. [Escape hatches: raw frames](#escape-hatches-raw-frames)
15. [Testing your bot](#testing-your-bot)
16. [Determinism](#determinism)
17. [Versioning and compatibility](#versioning-and-compatibility)

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

# 3. open the spectator at http://localhost:7878/ and click Start
```

If you want to ship a competitive bot fast, jump straight to the
[tactical toolkit](#tactical-toolkit) — it provides opt-in
`Tracker`/`Gunner`/`Helm`/`Evader` components and a high-level
`TacticalBot` you can subclass with a single `decide()` method.

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

## What you're building

A bot is a remote process that controls one ship. The server runs the
simulation; your bot reads a per-tick slice of the world and sends back
one `Command`. Strategy is yours; everything else — transport, framing,
handshake, dispatch — is the SDK's.

**Goal.** Be the last ship afloat. If the match clock runs out before
that happens, the survivor with the highest HP wins, with remaining
ammo as the tiebreaker. If every ship dies on the same tick, the match
is a draw.

**The decision loop.** Every tick the server hands you a filtered
`WorldView` — your own ship state, the contacts your sensors can see
right now, and any events you can perceive (your own hits, nearby
splashes). You return one `Command` with four levers:

- `throttle` — `[-1, 1]`, full reverse to full ahead.
- `rudder` — `[-1, 1]`, hard port to hard starboard.
- `sensor_mode` — `active` (gives you range, makes you visible to
  everyone) or `passive` (bearing-only, silent).
- `fire` (optional) — a bearing and a range. Subject to ammo and the
  per-gun cooldown.

The server arbitrates. Throttle and rudder get clamped, fire requests
are rejected with an `error` if the gun is on cooldown or the magazine
is empty, and any `Command` that arrives after the tick deadline is
dropped — the previous tick's controls persist.

**Behaviors you need to implement.** The SDK handles the plumbing.
The interesting choices are yours:

- **Navigation.** Pick a course each tick. The ship has finite
  acceleration and turn rate (see `ship_specs`); a hard rudder swing
  is not free, and reverse is slower than ahead.
- **Sensing strategy.** Active radar gives you contact ranges but
  paints a target on your back; passive is silent but only delivers
  bearings. Most bots switch between the two — when, and for how
  long, is up to you.
- **Contact tracking.** Contact `id`s are per-tick only. If you want
  to know "is that the same enemy I saw three ticks ago?", do the
  data association yourself (or use the toolkit's `Tracker`).
- **Fire control.** Decide when a contact is worth a shell, lead
  moving targets so the shell and the ship arrive at the same place
  (the SDK's `lead_target` helper does the math), and budget your
  ammo across a match that can run for thousands of ticks.
- **Threat reaction.** A `Hit` event means you just took damage; a
  `ShellSplash` nearby means someone shot at you and missed. Both
  are signal — break course, change sensor mode, or close the
  distance before you lose the ship.
- **Tick budget.** You have `deadline_ms` (default 80 ms) to return a
  `Command`. Heavy planning, blocking I/O, or sleeps inside the tick
  callback will overrun it, and your command is dropped.

A bot that drives in a circle and pings every few seconds can still
land hits. The gap between a naïve bot and a competitive one is
mostly about how thoughtfully it handles the list above.

---

## The world and its rules

The simulation is a single square arena. The defaults below are what
every example bot, replay, and screenshot in this repo assumes — the
server can be launched with different values, so read them back from
the `welcome` frame's `ship_specs` in your bot rather than hard-coding.

### World geometry

| Property      | Default       | Notes                                                          |
|---------------|---------------|----------------------------------------------------------------|
| Map size      | 1000 × 1000   | Square arena. Origin top-left, `+x` right, `+y` down.          |
| Tick rate     | 10 Hz         | Fixed `dt = 0.1 s`. Physics never uses the wall clock.         |
| Walls         | Hard          | A ship that runs into a wall stops dead and loses 2 HP.        |
| Match timeout | Server-config | On timeout, highest HP wins; ammo is the tiebreaker.           |

There is no terrain, no obstacles, no fog of war beyond the sensor
model. Ships can occupy the same square (they don't collide with each
other) — only walls and shell splashes deal damage.

### Ship

| Spec                  | Default | Meaning                                                       |
|-----------------------|---------|---------------------------------------------------------------|
| `hull_hp`             | 100     | Damage to zero is death. No regeneration.                     |
| `max_forward_speed`   | 9.0     | Units per second at `throttle = 1`.                           |
| `max_reverse_speed`   | 2.0     | Units per second at `throttle = -1`.                          |
| `acceleration`        | 3.5     | Units per second². Full-stop to full-ahead takes ≈ 2.6 s.     |
| `turn_rate_deg_per_s` | 20.0    | At full rudder; scales linearly with `|speed| / max_forward`. |
| `max_ammo`            | 250     | Shells per match. No reload.                                  |

Read these from the `ship_specs` block on `welcome` rather than
hard-coding them; the server is authoritative and may run with
different values in tournament configurations.

A ship dead in the water can't turn — yaw rate scales with speed. If
you stall, you're a duck.

### Weapons and damage

| Spec                 | Default | Meaning                                                       |
|----------------------|---------|---------------------------------------------------------------|
| `shell_speed`        | 70.0    | Units per second. Roughly 8× the ship's top speed.            |
| `max_shell_range`    | 300.0   | Server-side clamp on the `fire.range` field.                  |
| `gun_cooldown_ticks` | 15      | 1.5 s between shots at the default tick rate.                 |
| `splash_radius`      | 15.0    | Damage falloff distance.                                      |
| `max_splash_damage`  | 25      | HP dealt to a ship sitting on the splash centre.              |

**There is no direct-hit damage.** Every shell flies its requested
distance, then detonates at end-of-flight and applies splash to every
ship (yours included — **friendly fire is on**) within the splash
radius. Damage scales linearly with distance from the centre:

- on the centre: 25 HP (a quarter of a hull)
- half a radius away (~7.5 units): ~13 HP
- on the edge or beyond (≥ 15 units): 0

So you don't aim *at* a ship — you aim at the patch of water where
the ship will be when the shell arrives. Roughly four perfectly
centred shots sink a target; most hits land off-centre and take 5–15
HP, so plan on spending the majority of your magazine.

---

## What you see each tick

Your `WorldView` is a *filtered* snapshot — never ground truth. It
has three pieces.

### Your own ship

The only ground-truth slice you get. Every tick: position, heading,
current speed, HP, ammo, and your last commanded rudder and throttle.
You also get the server's `deadline_ms` — the budget you have to
return a `Command` before this tick's frame is dropped.

### Contacts

What your sensors picked up *this tick only*. Each contact carries:

| Field         | Meaning                                                              |
|---------------|----------------------------------------------------------------------|
| `kind`        | `ship`, `shell`, or `unknown`.                                       |
| `pos`         | Best-estimate position. Sensor-specific noise applied.               |
| `bearing_deg` | Compass bearing from you to the contact.                             |
| `range`       | Distance — only present in active mode; passive contacts omit it.    |
| `confidence`  | `[0, 1]`. High in active, lower in passive.                          |
| `id`          | A per-tick string. Useless across ticks — do your own association.   |

You never see another ship's HP, ammo, sensor mode, or heading. If
you want to know whether the enemy is bleeding, you have to count the
hits *you* landed on it.

Which ships end up in `contacts` depends on the sensor mode you
commanded last tick:

| Sensor mode         | Detects                                                | Range | Noise                |
|---------------------|--------------------------------------------------------|-------|----------------------|
| `active`            | Every alive ship, regardless of their mode.            | 350   | ±2 units on position |
| `passive` (sweep)   | Any ship currently in `active` mode (loud).            | 500   | ±5° on bearing       |
| `passive` (nearby)  | Any ship at all (engine noise, close in).              | 150   | ±5° on bearing       |

The two passive rules union: in passive you hear active-pinging ships
out to 500 units and *all* ships out to 150, bearing-only.

A passive contact carries a `pos` field with a placeholder projection
just so the wire frame keeps a consistent shape — don't treat it as a
real position estimate. Use `bearing_deg` and gate on `range is None`.

### Events

Things that happened in your vicinity this tick. Bots see two kinds:

- `Hit` — *you* just took damage, with the HP amount.
- `ShellSplash` — a shell exploded within sensor range. The bot
  doesn't get to see whose shell it was, only where it went off.

You don't get a `Death` event for yourself — when your HP reaches
zero, the next message you receive is `game_over`. Other ships'
deaths manifest as their contacts going away.

---

## How a match flows

Every bot connection follows the same lifecycle. The SDK drives all of
it for you — the table below is for understanding *what your callbacks
see and when*.

| # | Direction | Frame        | What the SDK does                                                                 |
|---|-----------|--------------|-----------------------------------------------------------------------------------|
| 1 | bot → srv | `hello`      | Sent automatically when you call the SDK's `run` entry point.                     |
| 2 | srv → bot | `welcome`    | SDK parses it, stores it on the bot, fires `on_welcome`, sends `ready`.           |
| 3 | srv → bot | `game_start` | SDK fires `on_game_start`.                                                        |
| 4 | srv → bot | `tick` …     | SDK fires `on_tick` and sends your returned `Command` back.                       |
| 5 | srv → bot | `game_over`  | SDK fires `on_game_over`. By default the bot stays connected.                     |
| 6 | srv → bot | `lobby`      | Sent ~2s later, after the post-game pause. SDK fires `on_lobby`, sends `ready` again. |
| 7 | →          | (loop)       | Steps 3–6 repeat for every match the operator starts.                             |

Between (2) and (3) the server is in **lobby**: it waits for *all*
connected bots to be ready before starting. Your bot can connect any
time and will simply idle until `game_start` fires.

**Bots persist across matches on a single connection.** `bot_id` and
`ship_id` are stable for the lifetime of the WebSocket. After
`game_over` the SDK does **not** close the connection by default — it
waits for the server's `lobby` frame, auto-sends `ready`, and your bot
participates in the next match.

If you want one-game-per-process behaviour instead, return `False`
from `on_game_over`. The SDK will close the connection cleanly and
your `run` call will return.

The server is authoritative on every aspect of the simulation. Your
`Command` is a *request* — throttle and rudder get clamped to `[-1, 1]`,
fire requests get rejected with an `error` frame if the gun is on
cooldown or out of ammo, and command frames that arrive after the
tick's deadline are dropped (your previous controls persist).

If your tick callback throws, the SDK logs the exception and sends a
hold-station command instead — the connection stays open.

The full wire protocol — frame shapes, field semantics, error codes —
lives in [`../docs/PROTOCOL.md`](../docs/PROTOCOL.md).

---

## Coordinates, bearings, and units

- World coordinates: origin top-left, **+x right**, **+y down** (canvas
  convention).
- Bearings: **0° points along -y** (up on screen), **90° along +x**
  (right). Increase clockwise. Range `[0, 360)`.
- Distances, speeds, headings, rudders, throttles are floating-point.
  HP, ammo, and ticks are integer.
- Tick rate is set by the server (default `--tick-hz 10`, so
  `dt = 0.1s`).

The server's bearing convention is **not** the math-textbook one. Use
the SDK's `bearing_to` helper rather than hand-rolling `atan2` — the
helper returns the value the server expects.

---

## Base API reference

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

#### `Command.fire_at(target_pos, *, shooter_pos=None, target_vel=None, shell_speed=70.0, range=None, lead=True)`

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

Most useful fields (current defaults shown — always read them from
`welcome.ship_specs` rather than hard-coding): `shell_speed` (70.0),
`max_shell_range` (300.0), `gun_cooldown_ticks` (15), `hull_hp` (100),
`max_ammo` (250), `max_forward_speed` (9.0),
`turn_rate_deg_per_s` (20.0).

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

### `run(bot, *, host="localhost", port=7878, name="bot", version="naval-sdk/0.1.0", path="/bot") -> Optional[GameOver]`

Synchronous entry point. Wraps `asyncio.run()`. Returns the `GameOver`
payload if the match completed, else `None`.

### `run_async(...) -> Optional[GameOver]`

The same thing, but as a coroutine. Use it if your bot already runs
inside an async program.

---

## Tactical toolkit

Opt-in helpers for bot authors who want to focus on *strategy* (when to
engage, when to break off, what to prioritize) instead of the mechanics
of contact tracking, lead calculation, steering, and evasion.

Design rationale and trade-offs live in
[`../docs/design-decisions/sdk-tactical-toolkit.md`](../docs/design-decisions/sdk-tactical-toolkit.md);
this section is the user-facing reference.

### Layered architecture

Pick the layer you want to live at. You can drop down at any time.

| Layer | What you write                                                    | What the SDK gives you                                         |
|-------|-------------------------------------------------------------------|----------------------------------------------------------------|
| L0    | Subclass `Bot`, override `on_tick(view) → Command`.               | Wire protocol, message loop, raw helpers.                      |
| L1    | Same as L0, plus call stateless helpers.                          | `bearing_to`, `distance`, `lead_target`, `signed_bearing_delta`, `clamp`, `wrap_bearing`. |
| L2    | Subclass `Bot`, instantiate L2 components, compose them yourself. | `Tracker`, `Gunner`, `Helm`, `SensorPolicy`, `Evader`.         |
| L3    | Subclass `TacticalBot`, override `decide(ctx) → Intent`.          | Everything from L2 wired together with documented preemption.  |

You can replace any L2 component with your own implementation — they're
just classes implementing small, documented protocols.

### Layer 2 component reference

All L2 components live in `naval_sdk.tactical` and are independently
usable.

#### `Tracker`

Per-tick association + smoothing of `Contact` reports into persistent
`Track` objects with stable IDs, smoothed position, and windowed
velocity.

```python
tracker = Tracker(welcome.ship_specs, tick_hz=welcome.tick_hz)
...
tracks = tracker.update(view)  # call once per tick
for t in tracks:
    if t.kind == "ship":
        ...
```

| Knob                          | Default | What it controls                                                  |
|-------------------------------|---------|-------------------------------------------------------------------|
| `active_gate`                 | `60.0`  | Max position distance to fold an active contact into a track.     |
| `passive_bearing_gate_deg`    | `20.0`  | Max bearing delta to fold a passive (range-less) contact.         |
| `velocity_alpha`              | `0.3`   | EMA weight for new velocity samples.                              |
| `velocity_window_ticks`       | `10`    | Active-observation history used to baseline velocity estimates.   |
| `staleness_ticks`             | `40`    | A track without any observation for this long is dropped.         |

A `Track` exposes `pos` (current best estimate, predicted to the current
tick), `observed_pos` (last raw active measurement), `vel`,
`last_seen_tick`, `last_active_tick`, `confidence`, and a `source` of
`"active"`, `"passive"`, or `"dead_reckoned"`.

Passive (bearing-only) contacts can only fold into an *existing* track —
they never spawn new ones. This matches the design constraint that
bearing alone is too ambiguous to seed from.

#### `Gunner`

Cooldown, ammo accounting, lead-target solving, range and time-of-flight
feasibility, and a self-splash guard.

```python
gunner = Gunner(welcome.ship_specs)
...
gunner.attempt(cmd, view.me, target_track, view)  # mutates cmd in place
```

For finer control, `solve(...) → FireSolution | None` is side-effect-free;
call `note_fired(view.tick)` when you actually attach the resulting
`FireCommand`. `to_fire_command(sol)` converts a solution to the wire
type.

#### `Helm`

Translates a desired bearing or waypoint into `(throttle, rudder)` that
respects the speed-coupled turn rate. Sharp turns get reduced throttle.

```python
throttle, rudder = helm.steer_to_bearing(view.me, target_bearing_deg)
throttle, rudder = helm.steer_to_point(view.me, (x, y))
```

`steer_to_bearing(..., respect_walls=True)` overrides the target bearing
toward an inward direction if the ship is inside the configured
`wall_margin`. Pass `respect_walls=False` to disable.

#### `SensorPolicy`

Plug-in interface; ships with four implementations:

- `AlwaysActive()` — always active radar.
- `AlwaysPassive()` — always silent.
- `DutyCycle(active_ticks=10, passive_ticks=20)` — fixed cadence.
- `PingWhenStale(stale_threshold_ticks=15)` — passive while the freshest
  track is fresh; active otherwise (including when the track set is empty).

```python
class SensorPolicy(Protocol):
    def choose(self, view: WorldView, tracker: Tracker) -> SensorMode: ...
```

Implement your own by writing a class with a `choose(view, tracker)`
method.

#### `Evader`

Hit-triggered evasion state machine. Returns an override `Command` while
evading; `None` otherwise. The first hit picks a rudder sign; subsequent
hits during the cooldown flip the sign so a watching shooter can't reuse
their lead solution.

```python
evader = Evader(evasion_ticks=15, cooldown_ticks=10)
...
override = evader.update(view)
if override is not None:
    return override  # preempts everything else
```

### Layer 3: `TacticalBot`

```python
from naval_sdk import run
from naval_sdk.tactical import Intent, TacticalBot, TacticalContext

class MyBot(TacticalBot):
    def decide(self, ctx: TacticalContext) -> Intent:
        if ctx.threats:
            return Intent.engage(ctx.threats.nearest())
        return Intent.patrol(rect=(200, 200, 600, 600))

if __name__ == "__main__":
    run(MyBot())
```

#### Preemption order

Each tick, `TacticalBot.on_tick` evaluates the layers in this order —
higher items override lower ones:

1. **`Evader`** — if a hit just landed and the evader is active, its
   override `Command` is used directly (with the sensor policy's mode
   overlaid).
2. **`Intent.custom(cmd)`** — if your `decide()` returns a custom
   command, it passes through unchanged.
3. **Player `Intent`** — translated to `(throttle, rudder)` via `Helm`.
4. **`SensorPolicy`** — overlaid onto whatever movement command was
   chosen.
5. **`Gunner`** — opportunistically attached if a vetted shot exists.

#### `Intent` variants

| Constructor               | Behavior                                                                          |
|---------------------------|-----------------------------------------------------------------------------------|
| `Intent.engage(track)`    | Steer toward the track; fire when the gunner approves.                            |
| `Intent.patrol(rect)`     | Cycle the corners of `(x1, y1, x2, y2)`; fire opportunistically on nearest threat.|
| `Intent.retreat_to(point)`| Steer to a fixed point; fire opportunistically on nearest threat.                 |
| `Intent.hold()`           | Throttle 0, rudder 0; never fire.                                                 |
| `Intent.custom(cmd)`      | Use the given `Command` verbatim for this tick.                                   |

#### Swapping subsystems

Override `on_tactical_welcome(welcome)` to replace defaults:

```python
class MyBot(TacticalBot):
    def on_tactical_welcome(self, welcome):
        self.tracker = MyCustomTracker(welcome.ship_specs)
        self.sensor_policy = DutyCycle(active_ticks=5, passive_ticks=15)
```

### Cookbook: the same bot, three layers

A bot that closes on the nearest enemy and fires when possible.

**Layer 0 — bare protocol**

```python
class CloseAndFire(Bot):
    def on_welcome(self, welcome):
        self.shell_speed = welcome.ship_specs.shell_speed
        self.cooldown = welcome.ship_specs.gun_cooldown_ticks
        self.next_fire = 0

    def on_tick(self, view):
        cmd = Command(throttle=1.0, rudder=0.0, sensor_mode="active")
        target = view.nearest_contact()
        if target is None:
            return cmd
        delta = ((bearing_to(view.me.pos, target.pos) - view.me.heading_deg + 540) % 360) - 180
        cmd.rudder = max(-1.0, min(1.0, delta / 30.0))
        if view.me.ammo > 0 and view.tick >= self.next_fire:
            cmd.fire_at(target.pos, shooter_pos=view.me.pos, shell_speed=self.shell_speed)
            self.next_fire = view.tick + self.cooldown
        return cmd
```

**Layer 2 — composed subsystems**

```python
class CloseAndFire(Bot):
    def on_welcome(self, welcome):
        self.tracker = Tracker(welcome.ship_specs)
        self.gunner = Gunner(welcome.ship_specs)
        self.helm = Helm(welcome.ship_specs,
                         map_width=welcome.map.width,
                         map_height=welcome.map.height)

    def on_tick(self, view):
        ships = [t for t in self.tracker.update(view) if t.kind == "ship"]
        if not ships:
            return Command(throttle=1.0, rudder=0.0)
        target = min(ships, key=lambda t: distance(view.me.pos, t.pos))
        throttle, rudder = self.helm.steer_to_point(view.me, target.pos)
        cmd = Command(throttle=throttle, rudder=rudder)
        self.gunner.attempt(cmd, view.me, target, view)
        return cmd
```

**Layer 3 — pure tactical intent**

```python
class CloseAndFire(TacticalBot):
    def decide(self, ctx):
        if ctx.threats:
            return Intent.engage(ctx.threats.nearest())
        return Intent.hold()
```

### Choosing a layer

- **First match, or you just want to win quickly?** Start at L3
  (`TacticalBot`). Override `decide()`, run it. ~30 lines.
- **Want bespoke movement (orbits, kiting, formation) or bespoke fire
  selection (range bands, expected-damage gating)?** Drop to L2. Pick
  the components you want, write the orchestrator yourself.
- **Want to write your own tracker, fire control, or steering, or
  benchmark the SDK against your hand-rolled code?** Stay at L0/L1 — the
  base `Bot` class never went away.

See `examples/` for one bot at each layer: `circle_bot.py` (L0),
`tactician_bot.py` (L2), `strategist_bot.py` (L3).

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

More runnable bots live in [`../examples/`](../examples/), one per
tactical layer.

---

## Errors and how to react

Every time the server rejects something your bot did, it sends an
`error` frame: a short `code` string and a human-readable `message`.
Branch on `code` for behaviour, surface `message` for diagnostics.

`Bot.on_error(code, message)` exposes this. The default implementation
just logs the frame; override it only if you want custom counters,
alerts, or to close the connection on a specific code.

### Gameplay errors — bot stays connected

These mean "the game refused what you asked for." They are *not*
protocol violations and don't count toward disconnection. Most bots
can ignore them; you only need to handle them if your decision logic
depends on whether a specific action took effect.

| Code              | Trigger                                                                | Effect                                                              |
|-------------------|------------------------------------------------------------------------|---------------------------------------------------------------------|
| `late_command`    | Your `command` arrived after `deadline_ms` for that tick.              | Previous tick's throttle / rudder / sensor persist; no shot fires.  |
| `stale_command`   | Your `command.tick` was outside `[world_tick − 1, world_tick + 1]`.    | The command is dropped entirely.                                    |
| `cooldown_active` | You issued `fire` while the gun was still cooling down.                | Movement / sensor changes still apply; no shell spawns.             |
| `no_ammo`         | You issued `fire` with an empty magazine.                              | Movement / sensor changes still apply; no shell spawns.             |

`late_command` is the one to watch in development — if you see it
under load, your tick handler is doing too much work or blocking on
I/O. `cooldown_active` and `no_ammo` are easy to avoid by tracking
`gun_cooldown_ticks` and `view.me.ammo` yourself before issuing
`fire`.

### Protocol violations — five strikes and you're out

These mean "your message was malformed." The server replies with the
error *and* bumps an internal violation counter. After **5
violations** on a single connection, the server sends
`too_many_violations` and closes the WebSocket with code
`Policy (1008)`. The SDK builds every frame for you, so under normal
use you'll never see these — they only fire if you're using the raw
escape hatches (`raw_send`) and producing malformed JSON.

| Code                        | Trigger                                                                                                                          |
|-----------------------------|----------------------------------------------------------------------------------------------------------------------------------|
| `malformed_json`            | The frame didn't parse as JSON.                                                                                                  |
| `invalid_message`           | JSON parsed but didn't match any known schema, *or* you sent a non-`hello` frame before completing the handshake.                |
| `non_finite_value`          | A float field (`throttle`, `rudder`, `bearing_deg`, `range`) was `NaN` or `±Infinity`.                                           |
| `binary_frames_unsupported` | You sent a WebSocket binary frame. The `/bot` endpoint is text-only.                                                             |
| `too_many_violations`       | The fifth violation on this connection. The server closes the WebSocket immediately after sending this; the SDK will not retry. |

If you do see one, sanity-check the payload against
[`../docs/PROTOCOL.md`](../docs/PROTOCOL.md).

### Connection-lifecycle errors — connection dies immediately

These can fire before or instead of a match. After sending the error
the server closes the WebSocket with code `Policy (1008)`. The SDK
does not auto-reconnect.

| Code                | Trigger                                                                                            |
|---------------------|----------------------------------------------------------------------------------------------------|
| `handshake_timeout` | You connected but didn't send `hello` before the server's handshake deadline (5 s by default).     |
| `invalid_name`      | `hello.name` was empty, longer than 32 bytes, or contained anything outside `[A-Za-z0-9 _-]`.      |

Two more setup-time rejections surface differently — be aware so you
don't go looking for codes that never arrive:

- **Duplicate name.** If another live bot in the same room already
  holds your `name`, the server rejects registration with
  `invalid_message` (the `message` field carries the reason) and
  closes the connection. Pick a different `name` argument to `run`
  and retry.
- **Per-IP connection cap.** If the server is configured with a
  `--max-connections-per-ip` limit and your IP is at that cap, the
  TCP stream is dropped *before* the WebSocket handshake. The bot
  sees a connection-refused / immediate close with no error frame
  at all. Wait for existing connections to drain, or ask the
  operator to raise the cap.

---

## Common pitfalls

- **Forgetting your own position when firing** — the SDK's `fire_at`
  helper needs your ship's position as the shooter. Without it, the
  bearing is computed from the origin and you'll shoot the wrong way.
- **Hand-rolled bearings** — `atan2(dy, dx)` gives radians from +x. The
  server wants compass degrees from -y, clockwise. Use the SDK helper.
- **Passive contacts have no range** — sensor mode `passive` returns
  bearing-only contacts. Guard against missing range before doing math
  on it.
- **Active mode is loud** — anyone on the map can see your bearing while
  you're pinging, regardless of distance. Don't camp on `active` unless
  you mean to.
- **Stable contact IDs are a myth** — a contact's `id` is per-tick. To
  track an enemy across ticks, key on position/bearing similarity
  yourself (or use the toolkit's `Tracker`).
- **Tick deadline is real** — the default is 80 ms. If your tick
  callback blocks longer (heavy planning, I/O, sleeps), your command is
  dropped and the previous tick's controls persist.

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

`raw_send` / `raw_recv` are async because they hit the live socket. They
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

## Determinism

**Bot code runs in your process, not the server.** Replay fidelity is
the server's concern — it replays the recorded *command stream*, which
is identical regardless of how your bot produced those commands.

You are therefore free to use whatever data structures, randomness, or
floating-point precision you like in your bot. The server's
`BTreeMap` / no-`thread_rng` / fixed-`dt` rules apply only inside
`server/src/sim/`.

The tracker's defaults are stable as of SDK version `0.1.0`. Tweaking
them changes which contacts merge across runs — that's normal and
expected, not a determinism violation.

---

## Versioning and compatibility

- The SDK has its own artifact version (in `pyproject.toml`).
  `naval_sdk.__version__` exposes it at runtime. The wire protocol
  version is a separate string sent by the server in the `welcome`
  frame.
- **Additive** server changes (new optional fields, new event types)
  parse but are ignored by older SDKs — your bot keeps working.
- **Breaking** server changes bump the version string and are
  documented in [`../docs/PROTOCOL.md`](../docs/PROTOCOL.md) under the
  Changelog section. Pin the SDK version alongside your bot if you care
  about reproducibility.

---

## See also

- [`../docs/PROTOCOL.md`](../docs/PROTOCOL.md) — full wire protocol
  spec (frames, fields, error codes).
- [`../system-design.md`](../system-design.md) — system design, trust
  model, replay semantics.
- [`../examples/`](../examples/) — runnable example bots covering each
  tactical layer.
