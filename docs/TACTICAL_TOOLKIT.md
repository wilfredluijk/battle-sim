# Tactical Toolkit

Opt-in helpers for bot authors who want to focus on *strategy* (when to
engage, when to break off, what to prioritize) instead of the mechanics of
contact tracking, lead calculation, steering, and evasion.

Available in both supported SDKs with the same shape:

| Language | Package                            |
|----------|------------------------------------|
| Python   | `naval_sdk.tactical`               |
| Java     | `com.battlesim.naval.tactical`     |

This document uses Python in the code snippets for brevity; every type and
method has a 1:1 Java equivalent under the package above. Java idioms
(records vs. dataclasses, `Optional<T>` vs. nullable, sealed interfaces vs.
tagged unions) are noted inline where they differ.

Design rationale and trade-offs live in
[`design-decisions/sdk-tactical-toolkit.md`](./design-decisions/sdk-tactical-toolkit.md);
this document is the user-facing reference.

---

## Layered architecture

Pick the layer you want to live at. You can drop down at any time.

| Layer | What you write                                                    | What the SDK gives you                                         |
|-------|-------------------------------------------------------------------|----------------------------------------------------------------|
| L0    | Subclass `Bot`, override `on_tick(view) → Command`.               | Wire protocol, message loop, raw helpers.                      |
| L1    | Same as L0, plus call stateless helpers.                          | `bearing_to`, `distance`, `lead_target`, `signed_bearing_delta`, `clamp`, `wrap_bearing`. |
| L2    | Subclass `Bot`, instantiate L2 components, compose them yourself. | `Tracker`, `Gunner`, `Helm`, `SensorPolicy`, `Evader`.         |
| L3    | Subclass `TacticalBot`, override `decide(ctx) → Intent`.          | Everything from L2 wired together with documented preemption.  |

You can replace any L2 component with your own implementation — they're just
classes implementing small, documented protocols.

---

## Layer 2 component reference

All L2 components live in `naval_sdk.tactical` and are independently usable.

### `Tracker`

Per-tick association + smoothing of `Contact` reports into persistent
`Track` objects with stable IDs, smoothed position, and windowed velocity.

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

### `Gunner`

Cooldown, ammo accounting, lead-target solving, range and time-of-flight
feasibility, and a self-splash guard.

```python
gunner = Gunner(welcome.ship_specs)
...
gunner.attempt(cmd, view.me, target_track, view)  # mutates cmd in place
```

For finer control, `solve(...) → FireSolution | None` is side-effect-free;
call `note_fired(view.tick)` when you actually attach the resulting
`FireCommand`. `to_fire_command(sol)` converts a solution to the wire type.

### `Helm`

Translates a desired bearing or waypoint into `(throttle, rudder)` that
respects the speed-coupled turn rate. Sharp turns get reduced throttle.

```python
throttle, rudder = helm.steer_to_bearing(view.me, target_bearing_deg)
throttle, rudder = helm.steer_to_point(view.me, (x, y))
```

`steer_to_bearing(..., respect_walls=True)` overrides the target bearing
toward an inward direction if the ship is inside the configured
`wall_margin`. Pass `respect_walls=False` to disable.

### `SensorPolicy`

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

Implement your own by writing a class with a `choose(view, tracker)` method.

### `Evader`

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

---

## Layer 3: `TacticalBot`

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

### Preemption order

Each tick, `TacticalBot.on_tick` evaluates the layers in this order — higher
items override lower ones:

1. **`Evader`** — if a hit just landed and the evader is active, its
   override `Command` is used directly (with the sensor policy's mode
   overlaid).
2. **`Intent.custom(cmd)`** — if your `decide()` returns a custom command,
   it passes through unchanged.
3. **Player `Intent`** — translated to `(throttle, rudder)` via `Helm`.
4. **`SensorPolicy`** — overlaid onto whatever movement command was chosen.
5. **`Gunner`** — opportunistically attached if a vetted shot exists.

### `Intent` variants

| Constructor               | Behavior                                                                          |
|---------------------------|-----------------------------------------------------------------------------------|
| `Intent.engage(track)`    | Steer toward the track; fire when the gunner approves.                            |
| `Intent.patrol(rect)`     | Cycle the corners of `(x1, y1, x2, y2)`; fire opportunistically on nearest threat.|
| `Intent.retreat_to(point)`| Steer to a fixed point; fire opportunistically on nearest threat.                 |
| `Intent.hold()`           | Throttle 0, rudder 0; never fire.                                                 |
| `Intent.custom(cmd)`      | Use the given `Command` verbatim for this tick.                                   |

### Swapping subsystems

Override `on_tactical_welcome(welcome)` to replace defaults:

```python
class MyBot(TacticalBot):
    def on_tactical_welcome(self, welcome):
        self.tracker = MyCustomTracker(welcome.ship_specs)
        self.sensor_policy = DutyCycle(active_ticks=5, passive_ticks=15)
```

---

## Cookbook: the same bot, three layers

A bot that closes on the nearest enemy and fires when possible.

### Layer 0 — bare protocol

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

### Layer 2 — composed subsystems

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

### Layer 3 — pure tactical intent

```python
class CloseAndFire(TacticalBot):
    def decide(self, ctx):
        if ctx.threats:
            return Intent.engage(ctx.threats.nearest())
        return Intent.hold()
```

---

## Determinism

**Code in this package runs in your bot process, not the server.** Replay
fidelity is the server's concern — it replays the recorded *command stream*,
which is identical regardless of how your bot produced those commands.

You are therefore free to use whatever data structures, randomness, or
floating-point precision you like in your bot. The server's `BTreeMap`/
no-`thread_rng`/fixed-`dt` rules apply only inside `server/src/sim/`.

The tracker's defaults are stable as of SDK version `0.1.0`. Tweaking them
changes which contacts merge across runs — that's normal and expected, not a
determinism violation.

---

## Choosing a layer

- **First match, or you just want to win quickly?** Start at L3
  (`TacticalBot`). Override `decide()`, run it. ~30 lines.
- **Want bespoke movement (orbits, kiting, formation) or bespoke fire
  selection (range bands, expected-damage gating)?** Drop to L2. Pick the
  components you want, write the orchestrator yourself.
- **Want to write your own tracker, fire control, or steering, or
  benchmark the SDK against your hand-rolled code?** Stay at L0/L1 — the
  base `Bot` class never went away.

See `examples/python/` for one bot at each layer: `circle_bot.py` (L0),
`tactician_bot.py` (L2), `strategist_bot.py` (L3). The Java examples in
`examples/java/` mirror the same three layers: `SimpleCircleBot` (L0),
`TrackingCircleBot`/`StrongTacticalBot` (L2), `StrategistBot` (L3).

---

## Java vs. Python: name and shape map

The shapes are identical; only the naming and idioms differ.

| Concept              | Python                                 | Java                                                       |
|----------------------|----------------------------------------|------------------------------------------------------------|
| Package              | `naval_sdk.tactical`                   | `com.battlesim.naval.tactical`                             |
| Tracker              | `Tracker(specs, tick_hz, ...)`         | `new Tracker(specs, tickHz)` or `new Tracker(specs, tickHz, new Tracker.Config())` |
| Track                | `Track` dataclass                      | `Track` class with getters (`track.pos()`, `track.vel()`, …) |
| Gunner               | `Gunner(specs, ...)`, `solve(...) -> FireSolution \| None` | `new Gunner(specs)`, `solve(...) -> Optional<FireSolution>` |
| Helm                 | `Helm(specs, ...)`                     | `new Helm(specs, helmConfig)`                              |
| Helm result          | tuple `(throttle, rudder)`             | `Helm.Steering` record (`.throttle()`, `.rudder()`)        |
| SensorPolicy         | `Protocol` (duck-typed)                | `@FunctionalInterface SensorPolicy`; built-ins as nested static classes (`SensorPolicy.AlwaysActive`, etc.) |
| Evader               | `Evader(...)`, `update(view) -> Command \| None` | `new Evader(...)`, `update(view) -> Optional<Command>` |
| Intent               | dataclass + factory classmethods       | sealed interface with records (`Intent.Engage`, `Intent.Patrol`, …) and `Intent.engage(...)`, etc. factories |
| TacticalContext      | dataclass                              | record                                                      |
| TacticalBot          | subclass and override `decide(ctx)`    | subclass and override `decide(ctx)`                        |

The Java SDK keeps the same default constants (active gate `60.0`, passive
bearing gate `20°`, velocity α `0.3`, velocity window `10`, staleness `40`
ticks) so behaviour is consistent across languages.
