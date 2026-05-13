# Design Decision: SDK Tactical Toolkit

**Status:** Proposed
**Date:** 2026-05-13
**Scope:** `sdk-python/` (with shape-compatibility expected for future SDKs)

---

## 1. Context

Today the SDK exposes the wire protocol almost 1:1. Bot authors get `Bot`, `WorldView`, `Command`, a few stateless helpers (`bearing_to`, `lead_target`, `distance`), and nothing else.

Looking at the existing example bots (`tracking_bot`, `powerful_bot`, `tactician_bot`), every non-trivial bot re-implements the same ~200 lines of plumbing before it can express a single tactical idea:

- Stitching per-tick `contact.id`s into persistent tracks.
- Smoothing velocity estimates from noisy positions.
- Folding passive bearing-only contacts into active-range tracks.
- Cooldown / ammo accounting.
- Bearing arithmetic (signed delta, ±180° wrap).
- Turn-rate-aware steering (turn rate is speed-coupled).
- Wall-margin avoidance.
- Hit-response evasion state machine.
- Active/passive sensor scheduling.

These are signal-processing and control-theory chores. They are not *tactics*. They are also exactly where determinism-irrelevant bugs hide (a bot author misjudging a gating threshold quietly degrades tracking quality and the bot just plays worse).

The hackathon is meant to be a contest of **strategy and tactics**, not a contest of who debugged their Kalman filter first.

## 2. Goals and non-goals

### Goals

- **Make the easy thing easy.** A competitive bot should be expressible in ~40 lines of tactical code.
- **Keep the hard thing possible.** A bot author who wants to write their own tracker, fire-control, or steering must still be able to — without forking the SDK.
- **Be additive.** Existing bots that use the L0 API today must continue to work unchanged.
- **Stay portable in shape.** When a second SDK (TS, Rust client) ships, the component shapes should translate mechanically.

### Non-goals

- We will not ship opinionated "named tactics" (`flank`, `kite`, `bait`). Those are the bot author's edge.
- We will not build pathfinding or a full Kalman filter — the map has no obstacles and EMA smoothing covers the noise model.
- We will not enforce or assume bot-side determinism. Replays replay the server's command log; the SDK is free to use any data structures or ordering it likes.
- We will not change the wire protocol. This is purely a client-side ergonomics layer.

## 3. Functional approach (what bot authors experience)

The SDK becomes a **layered onion**. Each layer is opt-in. A bot author picks the deepest layer they want to live at, and can always drop down.

### Layer 0 — Raw protocol *(exists today)*
`Bot`, `WorldView`, `Command`, `raw_send/recv`. Untouched.

### Layer 1 — Stateless helpers *(mostly exists today)*
`bearing_to`, `distance`, `lead_target`, plus a small handful of new helpers (`signed_bearing_delta`, `wrap_bearing`, `clamp`).

### Layer 2 — Stateful subsystems *(new, à la carte)*
Independent classes a bot author can mix into a custom `on_tick`:

| Component       | Responsibility                                                        |
|-----------------|-----------------------------------------------------------------------|
| `Tracker`       | Per-tick contacts → stable tracks with smoothed velocity and staleness |
| `Gunner`        | Cooldown, ammo, lead, range and time-of-flight feasibility, splash-safety |
| `Helm`          | Steer to bearing or waypoint; throttle modulation; wall margin       |
| `SensorPolicy`  | When to ping (duty-cycle, ping-when-stale, etc.)                     |
| `Evader`        | Hit-triggered evasive maneuver state machine                          |

Each component takes data in, returns advice out. None of them touch the WebSocket. They are pure decision helpers.

### Layer 3 — `TacticalBot` orchestrator *(new)*
A `Bot` subclass that wires the L2 components together. Bot author overrides:

```python
class MyBot(TacticalBot):
    def decide(self, ctx: TacticalContext) -> Intent:
        if ctx.threats:
            return Intent.engage(ctx.threats.nearest())
        return Intent.patrol(rect=(100, 100, 400, 400))
```

`Intent` is a tagged union: `engage(track)`, `patrol(rect)`, `retreat_to(point)`, `hold()`, and an escape hatch `custom(Command)` for one-tick overrides.

### Escape hatches

- At L3, `Intent.custom(cmd)` hands a raw `Command` to the server for one tick.
- At L2, any component can be replaced with a user-written one as long as it implements the documented protocol (`Tracker.update(view) -> list[Track]`, etc.).
- At L1/L0, the existing `Bot` base class is unchanged.

## 4. Technical approach

### 4.1 Module layout

```
sdk-python/naval_sdk/
  __init__.py            # re-exports the public surface
  bot.py                 # existing Bot, run, run_async  (unchanged)
  protocol.py            # existing wire types          (unchanged)
  helpers.py             # L1: stateless math helpers   (small additions)
  tactical/              # NEW package — all L2 + L3 lives here
    __init__.py
    tracker.py           # Tracker, Track
    gunner.py            # Gunner, FireSolution
    helm.py              # Helm
    sensor.py            # SensorPolicy + built-in policies
    evader.py            # Evader
    context.py           # TacticalContext (composition of the above)
    intent.py            # Intent tagged union
    bot.py               # TacticalBot
```

Keeping `tactical/` as a sub-package makes the L0/L1 surface unchanged and signals "this is opt-in."

### 4.2 Tracker — the keystone

State: `dict[int, Track]` keyed by an SDK-assigned stable id.

Per tick:
1. **Predict** each existing track forward by `dt = 1/tick_hz` using its last estimated velocity.
2. **Gate** incoming contacts against predictions:
   - Active contacts (range available): position distance gate (default `60` units, configurable).
   - Passive contacts (bearing-only): bearing gate (default `±20°`) against predicted bearing from own ship.
3. **Fold** matched contacts into existing tracks; **spawn** new tracks for unmatched ones.
4. **Update velocity** via EMA: `v_new = α * (Δp/Δt) + (1-α) * v_old`, default `α = 0.4`. Skip the update for passive-only folds (no position fix).
5. **Stale** any track not seen for `staleness_ticks` (default 40) — drop it.

Why EMA and not Kalman? With `±2u` uniform position noise and `dt = 0.1s`, EMA's tracking error settles at well under 1 u/s — adequate for `shell_speed = 50 u/s` lead solutions. Kalman is more code, more tuning, and the same answer in this regime.

### 4.3 Gunner

`solve(me, track, view) -> FireCommand | None`. Returns `None` (rather than raising) for any failure mode, so the calling code is just `if shot := gunner.solve(...): cmd.fire = shot`. Internally:

1. Cooldown elapsed? Ammo > 0?
2. Compute lead point with `helpers.lead_target`.
3. Lead point within `max_range` of own position?
4. Time-of-flight sanity (track won't leave range before shell arrives)?
5. Self-splash guard: aim point further than `splash_radius * margin` from own position?

Internal cooldown counter is updated when the caller actually fires; the SDK exposes `gunner.note_fired(tick)` for the rare bot that fires outside `solve()`.

### 4.4 Helm

`steer_to_bearing(me, target_bearing, *, max_turn_throttle=0.6) -> (throttle, rudder)`:
- `rudder = clamp(signed_bearing_delta(target, current) / 30°, -1, 1)`.
- `throttle = 1.0` when aligned within ±10°, scaling down to `max_turn_throttle` at large deltas. This works *with* the speed-coupled turn rate rather than against it.

`steer_to_point(me, point)` is a thin wrapper that computes the bearing first.

Optional `wall_margin` mode: if the next-tick projected position would be within `margin` of a boundary and the target bearing points further toward it, the helm overrides with a tangential bearing along the wall.

### 4.5 SensorPolicy

A small strategy interface with built-in implementations:

```python
class SensorPolicy(Protocol):
    def choose(self, view: WorldView, tracker: Tracker) -> SensorMode: ...

# Built-ins
AlwaysActive()
AlwaysPassive()
DutyCycle(active_ticks=10, passive_ticks=20)
PingWhenStale(stale_threshold_ticks=15)
```

Bot authors plug in their own by implementing the protocol.

### 4.6 Evader

State machine with three states: `IDLE`, `EVADING(until_tick, rudder_sign)`, `COOLDOWN(until_tick)`. Transitions:

- On `HitEvent` in `view.events` while `IDLE` or `COOLDOWN`: enter `EVADING` for `N` ticks, flip rudder sign if previous evasion ended recently.
- On `EVADING` expiry: enter `COOLDOWN` for `M` ticks.

`update(view) -> Command | None`: returns a high-priority override command while evading, else `None`.

### 4.7 TacticalBot and Intent

```python
class Intent:
    @staticmethod
    def engage(track: Track) -> "Intent": ...
    @staticmethod
    def patrol(rect: tuple[float, float, float, float]) -> "Intent": ...
    @staticmethod
    def retreat_to(point: Vec2) -> "Intent": ...
    @staticmethod
    def hold() -> "Intent": ...
    @staticmethod
    def custom(cmd: Command) -> "Intent": ...
```

`TacticalBot.on_tick(view)` (the framework method) does, in order:

1. `tracks = self.tracker.update(view)`
2. Build a `TacticalContext` from `view + tracks + specs`.
3. If evader has an override → use it directly (evasion preempts the player's intent).
4. Else `intent = self.decide(ctx)` (user code).
5. Translate intent → `Command` via the L2 components.
6. Overlay sensor mode from `SensorPolicy`.
7. Overlay fire from `Gunner` (intent-dependent: `engage` always tries to shoot, `patrol` only shoots opportunistically).

Steps 1, 3, 5, 6, 7 are mechanical; only step 4 is bot-author code.

### 4.8 Determinism note

The SDK runs in the bot process, not the server. Replays replay the server's command log byte-for-byte regardless of how the bot produced those commands. The SDK is therefore free to use `HashMap`, `random`, `time.time()`, or any non-deterministic structure. **This is worth calling out in the docs** so bot authors don't apply the server's determinism rules to their own code.

### 4.9 Backwards compatibility

The existing `Bot`, `WorldView`, `Command`, and helper exports stay at their current import paths. All new code is reachable through `naval_sdk.tactical.*` (or re-exported from `naval_sdk` for convenience). The `examples/circle_bot.py`, `examples/chaser_bot.py`, and `examples/sniper_bot.py` keep working without edits.

## 5. Alternatives considered

- **One monolithic "smart bot" framework.** Rejected: forces bot authors into a single design philosophy. The layered approach preserves choice.
- **Kalman-filter-based tracker.** Rejected for now: ROI doesn't justify the complexity at this noise level and tick rate. Could be added later as an alternative `Tracker` implementation behind the same interface.
- **Shipping named tactics (`flank`, `kite`).** Rejected: that's exactly the thing players should compete on.
- **Server-side targeting assist.** Rejected: would violate the trust-boundary invariant (server never makes decisions for the bot) and the sensor-filter invariant.

## 6. Risks

- **Track IDs become an API surface.** Once `Tracker` ships, tweaking gating defaults changes which contacts merge across releases. The defaults must be versioned, and changes called out in release notes.
- **Cross-SDK parity.** If we ship a TS/Rust SDK before the API has settled, we'll either lock the API too early or fragment. Mitigation: keep the L2 component shapes data-oriented (plain dataclasses, no Python-only patterns in signatures).
- **Documentation debt.** A layered SDK is only as good as the cookbook that teaches the layers. The plan budgets a non-trivial doc pass.
- **Hidden coupling between components.** `TacticalBot` wires five subsystems together. If two of them disagree (e.g., `Helm` wants to turn away from a wall, `Gunner` wants to hold heading for a shot), the resolution policy needs to be explicit. The orchestrator documents this priority order: `Evader > Helm-wall-override > Intent > Gunner > SensorPolicy`.
