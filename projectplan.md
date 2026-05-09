# Project Implementation Plan

Implementation roadmap for the naval battle simulator described in `system-design.md`. Steps are ordered so each one can be picked up by Claude Code in a single session and leaves the repo in a working, demoable state.

Conventions:
- Each step lists its **deliverable**, the **files touched**, and an **acceptance check** so completion is unambiguous.
- Earlier steps are prerequisites for later ones unless marked `[parallel]`.
- "Smoke test" = run by hand or via `cargo run` / `pytest`; full automated tests come later.

---

## Status

| Phase | Title | Status |
|---|---|---|
| 1 | Server skeleton | **complete** |
| 2 | Wire protocol | **complete** |
| 3 | World and physics | **complete** |
| 4 | Single-bot loop | **complete** |
| 5 | Sensors | **complete** |
| 6 | Combat | **complete** |
| 7 | Spectator | pending |
| 8 | Replay | pending |
| 9 | Python SDK | pending |
| 10 | Examples and onboarding | pending |
| 11 | Polish | pending |

---

## Phase 1 — Server skeleton  *(complete)*

### 1.1 Initialize Rust workspace and binary  *(done)*
- **Deliverable:** `server/Cargo.toml` with deps (`tokio`, `tokio-tungstenite`, `serde`, `serde_json`, `glam`, `rand`, `rand_pcg`, `clap`, `tracing`, `tracing-subscriber`); empty `src/main.rs` that prints a banner and exits.
- **Acceptance:** `cargo build` succeeds; `cargo run` prints the banner.

### 1.2 CLI argument parsing  *(done)*
- **Deliverable:** `clap`-based parser for the flags in §3.3 (`--port`, `--tick-hz`, `--tick-deadline-ms`, `--map`, `--max-bots`, `--seed`, `--replay-dir`); a `Config` struct passed into runtime.
- **Acceptance:** `cargo run -- --help` shows all flags; bad input produces a clear error.

### 1.3 WebSocket accept loop  *(done)*
- **Deliverable:** `src/net.rs` accepting connections on `/bot` and `/spectate`; each connection becomes a tokio task that reads frames and logs them via `tracing`.
- **Acceptance:** `wscat -c ws://localhost:7878/bot` connects; sent JSON appears in server logs.

### 1.4 Stdin control plane  *(done)*
- **Deliverable:** `src/control.rs` reading stdin line-by-line; recognizes a `quit` command and triggers graceful shutdown. Stub the rest of §3.3 commands as "not implemented" log lines.
- **Acceptance:** Typing `quit` in the terminal cleanly stops the server.

---

## Phase 2 — Wire protocol  *(complete)*

### 2.1 Protocol types  *(done)*
- **Deliverable:** `src/protocol.rs` with serde-tagged enums for every Bot↔Server message in §4.1 and the spectator `world` message in §4.2. Internally-tagged on `"type"`.
- **Acceptance:** `cargo test` round-trips each variant through `serde_json` to_string → from_str.

### 2.2 Protocol validation at the boundary  *(done)*
- **Deliverable:** `net.rs` parses incoming frames into `BotMsg`; malformed frames return an `error` message to the client and increment a per-connection violation counter (disconnect after N).
- **Acceptance:** Sending `{}` over `/bot` yields a typed error reply, not a server panic.

### 2.3 PROTOCOL.md  *(done)*
- **Deliverable:** `docs/PROTOCOL.md` mirroring §4 of the design doc with examples lifted from `protocol.rs`. Add a "Changelog" section header (empty for now).
- **Acceptance:** Field names in `protocol.rs` and `PROTOCOL.md` match exactly (eyeball + grep).

---

## Phase 3 — World and physics  *(complete)*

### 3.1 World data structures  *(done)*
- **Deliverable:** `src/sim/world.rs` with `World`, `Ship`, `Shell` structs using `glam::Vec2` (`f32`); constants module for the values in §5.2.
- **Acceptance:** A unit test constructs a world with two ships at known positions and reads back state.

### 3.2 Physics integration  *(done)*
- **Deliverable:** `src/sim/physics.rs` implementing throttle/rudder integration with the constants in §5.2; wall collision (stop + small bump damage).
- **Acceptance:** Unit tests: ship at full throttle reaches max speed; full rudder turn rate matches spec at top speed; wall hit clamps position and applies damage.

### 3.3 Tick loop scaffolding  *(done)*
- **Deliverable:** `src/room.rs` Room struct and `step_tick()` method calling physics; main runtime spawns one room called `main` on startup that ticks at `--tick-hz` with no bots.
- **Acceptance:** Server logs `tick=N` lines at the configured rate; tick numbers are monotonic.

---

## Phase 4 — Single-bot loop  *(complete)*

### 4.1 Lobby state and handshake  *(done)*
- **Deliverable:** Room state machine (`Lobby` / `Running` / `Ended`); `hello` → `welcome` exchange assigning a `bot_id` and `ship_id`; `ready` flag tracked per bot.
- **Acceptance:** A Python script connects, sends `hello`, receives `welcome` with assigned IDs, sends `ready`, receives no further messages until game start.

### 4.2 Operator `room start` command  *(done)*
- **Deliverable:** Stdin command `room start <name>` transitions the room to `Running` if all connected bots are ready; emits `game_start` with starting positions per §5.6.
- **Acceptance:** With one bot connected and ready, typing `room start main` causes the bot to receive `game_start`.

### 4.3 Tick → command exchange  *(done)*
- **Deliverable:** Each tick, the room sends every bot a `tick` message (self-state only for now, `contacts: []`); collects `command` replies until deadline; applies `throttle`/`rudder` sorted by `bot_id`.
- **Acceptance:** A scripted Python bot driving in a circle moves visibly in server logs (position changes per tick).

### 4.4 Late-command handling  *(done)*
- **Deliverable:** Commands arriving after deadline are dropped with an `error` reply; previous throttle/rudder/sensor_mode persist; missing commands do not disconnect the bot.
- **Acceptance:** Unit/integration test: bot deliberately delays one tick, server keeps it alive and reuses prior controls.

---

## Phase 5 — Sensors  *(complete)*

### 5.1 Active radar  *(done)*
- **Deliverable:** `src/sim/sensors.rs` with a function computing visible contacts within 350 units when `sensor_mode == "active"`; per-tick contact IDs (not ship IDs); position with seeded ±2 unit noise drawn from the room RNG.
- **Acceptance:** Unit test with two ships at 200 units apart sees one contact each when both active.

### 5.2 Passive listening  *(done)*
- **Deliverable:** Passive mode logic: detect actives within 500 units, anyone within 150 units; bearing-only with seeded ±5° noise.
- **Acceptance:** Unit test: silent ship at 400 units is invisible to passive listener; same ship while pinging is visible.

### 5.3 Wire sensors into tick payload  *(done)*
- **Deliverable:** Replace the empty `contacts: []` from 4.3 with the filtered output for each bot's chosen sensor mode; record per-bot sensor mode for the next tick's "who pinged last tick" logic.
- **Acceptance:** Two-bot smoke test: one active, one passive — passive bot sees the active one only at expected ranges.

---

## Phase 6 — Combat  *(complete)*

### 6.1 Firing and shells  *(done)*
- **Deliverable:** `src/sim/combat.rs` handling `fire` commands: spawn a `Shell` with bearing + requested range; enforce gun cooldown server-side; update ammo.
- **Acceptance:** Unit test: bot fires at bearing 90, range 200 — a shell exists in world state with correct velocity and TTL of 40 ticks.

### 6.2 Shell flight and splash damage  *(done)*
- **Deliverable:** Per-tick shell integration; on TTL expiry, apply linear-falloff damage within 15 units (25 dmg → 0 dmg); friendly fire on.
- **Acceptance:** Unit test: shell expires next to a stationary ship and lands the expected damage; a ship hit by its own shell takes damage.

### 6.3 Death and win condition  *(done)*
- **Deliverable:** Ships at HP ≤ 0 are removed; affected bots get `game_over`; room transitions to `Ended` when ≤1 alive; 3000-tick timeout with HP-then-ammo tiebreaker.
- **Acceptance:** Two scripted bots fight to completion; the loser receives `game_over`, the winner receives `game_over` with itself as winner.

### 6.4 Hit/splash events surfaced to bots  *(done)*
- **Deliverable:** Populate the `events` array in the `tick` payload with `hit` and `shell_splash` events the bot can perceive (own hits always; splashes within sensor range).
- **Acceptance:** Bot taking damage logs the `hit` event from its tick payload.

---

## Phase 7 — Spectator

### 7.1 Static file serving
- **Deliverable:** Server serves files from `spectator/` at `/`. Stub `spectator/index.html` says "hello".
- **Acceptance:** `curl http://localhost:7878/` returns the stub HTML.

### 7.2 Spectator world broadcast
- **Deliverable:** Each tick, the room sends the `world` message from §4.2 to every `/spectate` connection; full ground truth.
- **Acceptance:** `wscat -c ws://localhost:7878/spectate` prints a JSON `world` message every tick during a running match.

### 7.3 Canvas renderer
- **Deliverable:** `spectator/index.html` + `render.js` + `style.css`: canvas with map bounds, triangle ships colored per player, name + HP bar, shell dots, splash rings, sidebar with tick/players/events.
- **Acceptance:** Open the URL during a live match; see ships moving, shells flying, splashes on hit.

### 7.4 Active-radar visualization
- **Deliverable:** Renderer draws a faint translucent 350-unit circle around any ship whose last command had `sensor_mode == "active"`. Add `sensor_mode` to the spectator `world` payload if not already present.
- **Acceptance:** Switching a bot from passive to active shows/hides the ring in the browser.

---

## Phase 8 — Replay

### 8.1 Replay log writer
- **Deliverable:** `src/replay.rs` writes a JSONL file per match in `--replay-dir`: header line (seed, config, bot names) + one line per tick (tick number + sorted commands).
- **Acceptance:** A finished match leaves a `match_<timestamp>.jsonl` file with one header + N tick lines.

### 8.2 Replay playback flag
- **Deliverable:** `--replay <file>` flag re-runs the simulation from the log and broadcasts to spectators (no bot connections).
- **Acceptance:** Replaying a saved match in the browser is visually identical to the live run.

### 8.3 Replay determinism test
- **Deliverable:** `server/tests/replay_determinism.rs` runs a fixed-seed match, captures final world state, replays from the log, asserts byte-identical final state.
- **Acceptance:** `cargo test replay_determinism` passes.

---

## Phase 9 — Python SDK

### 9.1 Package skeleton
- **Deliverable:** `sdk-python/pyproject.toml`; `naval_sdk/__init__.py` exporting `Bot`, `WorldView`, `Command`, `run`; `pip install -e .` works.
- **Acceptance:** `python -c "import naval_sdk"` succeeds.

### 9.2 Connection and message loop
- **Deliverable:** `naval_sdk/bot.py` with `run()`: connects, handshakes, dispatches `on_welcome` / `on_tick` / `on_game_over`; `raw_send`/`raw_recv` escape hatches; logs and continues on malformed messages.
- **Acceptance:** A `Bot` subclass that returns `Command(throttle=1.0)` drives the ship forward against a live server.

### 9.3 Typed views and helpers
- **Deliverable:** `WorldView`, `Contact`, `SelfState` dataclasses; helpers `bearing_to`, `distance`, `lead_target`; `Command.fire_at(pos, lead=True)` using shell speed = 50.
- **Acceptance:** Unit tests for the math helpers; `Command.fire_at` produces the expected bearing for a moving target.

---

## Phase 10 — Examples and onboarding

### 10.1 `circle_bot.py`
- **Deliverable:** Drives in a circle, fires at random bearings.
- **Acceptance:** Runs against the server end-to-end without errors.

### 10.2 `chaser_bot.py`
- **Deliverable:** Active radar, naive pursuit of nearest contact.
- **Acceptance:** Beats `circle_bot` in a 1v1 most of the time.

### 10.3 `sniper_bot.py`
- **Deliverable:** Passive listening, only pings when committing to a shot, uses `lead_target`.
- **Acceptance:** Beats `chaser_bot` in a 1v1 most of the time.

### 10.4 QUICKSTART.md
- **Deliverable:** `docs/QUICKSTART.md`: install, start server, run a bot, open spectator — under 5 minutes from clone to first match.
- **Acceptance:** A teammate (or you, on a fresh clone) can follow it without other docs.

---

## Phase 11 — Polish

### 11.1 Two-bot integration test
- **Deliverable:** `server/tests/two_bot_match.rs` spins the server in-process, connects two scripted bots, runs to completion, asserts a winner is declared.
- **Acceptance:** `cargo test two_bot_match` passes; runs in CI.

### 11.2 Lint and format gate
- **Deliverable:** CI workflow (or a documented `just check` / `make check`) running `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `pytest`.
- **Acceptance:** Green run on a clean checkout.

### 11.3 Error message audit
- **Deliverable:** Walk every `error` reply path in `protocol.rs` / `net.rs` and ensure messages are actionable for bot authors (include tick number, expected vs actual where useful).
- **Acceptance:** Manual review; a bot author can debug without reading server source.

### 11.4 README
- **Deliverable:** Top-level `README.md`: one-paragraph pitch, link to QUICKSTART, link to PROTOCOL, link to system-design.
- **Acceptance:** A first-time visitor lands on the right doc within one click.

---

## Critical path

Phases 1 → 2 → 3 → 4 → 5 → 6 → 7 land a playable, watchable game. Phases 8–11 add replay, SDK ergonomics, and polish but the system is demoable without them.
