# Naval SDK guide

Shared concepts that apply to every **battle-sim** bot SDK — Python,
Java, and any future ports. Each SDK's own `README.md` covers
language-specific details (install, types, code samples); this document
covers the simulation contract those SDKs all sit on top of.

If you're new, read this once and then keep your language's README open
while you write your first bot.

---

## Table of contents

1. [What you're building](#what-youre-building)
2. [The world and its rules](#the-world-and-its-rules)
3. [What you see each tick](#what-you-see-each-tick)
4. [How a match flows](#how-a-match-flows)
5. [Coordinates, bearings, and units](#coordinates-bearings-and-units)
6. [Common pitfalls](#common-pitfalls)
7. [Versioning and compatibility](#versioning-and-compatibility)

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
  data association yourself: match on position, bearing, or a small
  motion model.
- **Fire control.** Decide when a contact is worth a shell, lead
  moving targets so the shell and the ship arrive at the same place
  (the SDK's `lead_target` / `leadTarget` helper does the math), and
  budget your 20 rounds across a match that can run for thousands of
  ticks.
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
| `max_forward_speed`   | 6.0     | Units per second at `throttle = 1`.                           |
| `max_reverse_speed`   | 2.0     | Units per second at `throttle = -1`.                          |
| `acceleration`        | 1.5     | Units per second². Full-stop to full-ahead takes ≈ 4 s.       |
| `turn_rate_deg_per_s` | 15.0    | At full rudder; scales linearly with `|speed| / max_forward`. |
| `max_ammo`            | 20      | Shells per match. No reload.                                  |

A ship dead in the water can't turn — yaw rate scales with speed. If
you stall, you're a duck.

### Weapons and damage

| Spec                 | Default | Meaning                                                       |
|----------------------|---------|---------------------------------------------------------------|
| `shell_speed`        | 50.0    | Units per second. Roughly 8× the ship's top speed.            |
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

Every bot connection follows the same five-step sequence. The SDK drives
all of it for you — the table below is for understanding *what your
callbacks see and when*.

| # | Direction | Frame        | What the SDK does                                                          |
|---|-----------|--------------|----------------------------------------------------------------------------|
| 1 | bot → srv | `hello`      | Sent automatically when you call the SDK's `run` entry point.              |
| 2 | srv → bot | `welcome`    | SDK parses it, stores it on the bot, fires the welcome callback, sends `ready`. |
| 3 | srv → bot | `game_start` | SDK fires the game-start callback.                                         |
| 4 | srv → bot | `tick` …     | SDK fires the tick callback and sends your returned `Command` back.        |
| 5 | srv → bot | `game_over`  | SDK fires the game-over callback once, then closes the connection.         |

Between (2) and (3) the server is in **lobby**: it waits for *all*
connected bots to be ready before starting. Your bot can connect any
time and will simply idle until `game_start` fires.

The server is authoritative on every aspect of the simulation. Your
`Command` is a *request* — throttle and rudder get clamped to `[-1, 1]`,
fire requests get rejected with an `error` frame if the gun is on
cooldown or out of ammo, and command frames that arrive after the
tick's deadline are dropped (your previous controls persist).

If your tick callback throws, the SDK logs the exception and sends a
hold-station command instead — the connection stays open.

The full wire protocol — frame shapes, field semantics, error codes —
lives in [`PROTOCOL.md`](PROTOCOL.md).

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
your SDK's `bearing_to` / `bearingTo` helper rather than hand-rolling
`atan2` — the helper returns the value the server expects.

---

## Common pitfalls

- **Forgetting your own position when firing** — the SDK's `fire_at` /
  `fireAt` helpers need your ship's position as the shooter. Without
  it, the bearing is computed from the origin and you'll shoot the
  wrong way.
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
  yourself.
- **Tick deadline is real** — the default is 80 ms. If your tick
  callback blocks longer (heavy planning, I/O, sleeps), your command is
  dropped and the previous tick's controls persist.

---

## Versioning and compatibility

- Each SDK has its own artifact version (in `pyproject.toml` or
  `pom.xml`). The wire protocol version is a separate string sent by
  the server in the `welcome` frame.
- **Additive** server changes (new optional fields, new event types)
  parse but are ignored by older SDKs — your bot keeps working.
- **Breaking** server changes bump the version string and are
  documented in [`PROTOCOL.md`](PROTOCOL.md) under the Changelog
  section. Pin the SDK version alongside your bot if you care about
  reproducibility.

---

## See also

- [`PROTOCOL.md`](PROTOCOL.md) — wire protocol spec (frames, fields,
  error codes).
- [`../system-design.md`](../system-design.md) — full system design,
  trust model, replay semantics.
- [`../sdk-python/README.md`](../sdk-python/README.md) — Python SDK.
- [`../sdk-java/README.md`](../sdk-java/README.md) — Java SDK.
