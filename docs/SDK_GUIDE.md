# Naval SDK guide

Shared concepts that apply to every **battle-sim** bot SDK — Python,
Java, and any future ports. Each SDK's own `README.md` covers
language-specific details (install, types, code samples); this document
covers the simulation contract those SDKs all sit on top of.

If you're new, read this once and then keep your language's README open
while you write your first bot.

---

## Table of contents

1. [How a match flows](#how-a-match-flows)
2. [Coordinates, bearings, and units](#coordinates-bearings-and-units)
3. [Common pitfalls](#common-pitfalls)
4. [Versioning and compatibility](#versioning-and-compatibility)

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
