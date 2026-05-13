# Implementation Plan: SDK Tactical Toolkit

Companion to [`design-decisions/sdk-tactical-toolkit.md`](./design-decisions/sdk-tactical-toolkit.md). High-level phases only — each phase is one PR-sized chunk of work.

---

## Phase 0 — Scaffolding (½ day)

- Create `sdk-python/naval_sdk/tactical/` package with empty modules and `__init__.py`.
- Add a small set of new stateless helpers (`signed_bearing_delta`, `wrap_bearing`, `clamp`) to `helpers.py`.
- Wire up package exports so `from naval_sdk.tactical import Tracker, Gunner, ...` resolves (even though the classes are still stubs).

**Exit criteria:** `pytest` still green, new imports resolve, no behavioral change.

---

## Phase 1 — Tracker + Gunner (highest ROI, 2–3 days)

- Implement `Track` dataclass and `Tracker` (predict → gate → fold → spawn → stale).
- Implement `FireSolution` and `Gunner` (cooldown, ammo, lead, range and ToF feasibility, splash-self guard).
- Unit tests:
  - `Tracker` associates a moving target across 30 ticks of active contacts with `±2u` injected noise.
  - `Tracker` folds passive bearing-only contacts into an existing active track.
  - `Tracker` stales tracks after the configured timeout.
  - `Gunner` returns `None` during cooldown, out of range, and when a self-splash would occur.
- Rewrite `examples/tracking_bot.py` to use `Tracker` + `Gunner`. Measure line-count delta.

**Exit criteria:** `tracking_bot.py` drops by ~60% in lines, behaves at least as well in a head-to-head against the old version.

---

## Phase 2 — Helm + SensorPolicy + Evader (2 days)

- Implement `Helm` (`steer_to_bearing`, `steer_to_point`, optional `wall_margin` override).
- Implement `SensorPolicy` protocol and the four built-in strategies (`AlwaysActive`, `AlwaysPassive`, `DutyCycle`, `PingWhenStale`).
- Implement `Evader` state machine.
- Unit tests for each:
  - `Helm` produces sensible throttle/rudder for various bearing deltas, including wall-margin trigger cases.
  - `DutyCycle` and `PingWhenStale` produce the expected mode sequence.
  - `Evader` enters/exits states correctly across hit events.
- Rewrite `examples/tactician_bot.py` to use the full L2 stack.

**Exit criteria:** `tactician_bot.py` reads as tactical code, not plumbing. Head-to-head parity or better.

---

## Phase 3 — TacticalBot orchestrator (2 days)

- Implement `Intent` tagged union and `TacticalContext`.
- Implement `TacticalBot` with the documented preemption order (`Evader > Helm-wall > Intent > Gunner > SensorPolicy`).
- Write `examples/strategist_bot.py` purely at L3 — target is ≤ 40 lines of tactical code.
- Integration test: spin up two `TacticalBot`s with different `decide()` implementations and run a full match through `server/tests/two_bot_match.rs`-style harness.

**Exit criteria:** A new bot author can write a competitive bot by overriding one method.

---

## Phase 4 — Documentation pass (1 day)

- Add a "Tactical Toolkit" section to `docs/SDK_GUIDE.md`.
- Add a cookbook page showing the same bot written at L0, L2, and L3 side-by-side.
- Add a short determinism note clarifying that bot-side code is not bound by the simulation's determinism rules.
- Update `docs/QUICKSTART.md` to point new players at `TacticalBot` as the recommended starting point.

**Exit criteria:** A reader who has never touched the SDK can pick the right layer for their skill level from the docs alone.

---

## Phase 5 — Polish and release (½ day)

- Version-bump the SDK; note in the changelog which `Tracker` defaults are public API.
- Confirm `circle_bot`, `chaser_bot`, `sniper_bot` (L0 bots) still run unchanged.
- Tag the release and announce.

**Exit criteria:** SDK releases cleanly, all example bots compile and run, CI green.

---

## Out of scope (deferred)

- Cross-language SDK port (TS/Rust). Revisit once the Python API has lived through one hackathon and stabilized.
- Kalman-filter `Tracker` variant. Add only if EMA proves insufficient in practice.
- Replay-analysis tooling. Belongs in a separate `tools/` effort, not the runtime SDK.
