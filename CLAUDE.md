# CLAUDE.md

Guidance for Claude when working in this repository. Read this before making non-trivial changes.

---

## What this project is

A hackathon programming game. Players write bots in any language, connect them to a central Rust server over WebSocket, and battle in a deterministic top-down naval simulation. A browser spectator renders matches live. The full system design lives in `system-design.md` (repo root) — read it first if you're new to the repo.

**Three components, three trust levels:**
- `server/` — Rust, authoritative, trusted. Owns all simulation state.
- `sdk-python/` (and any future SDKs) — convenience layer for bot authors. Untrusted from the server's perspective.
- `spectator/` — static HTML/JS, read-only viewer.

---

## Non-negotiable invariants

These rules exist because violating them silently breaks replays, fairness, or both. **Do not relax them without an explicit discussion.**

### Determinism in the simulation

The simulation must produce bit-identical results given the same seed and command log. This means inside `server/src/sim/` and anything it calls:

- **No `thread_rng()`, no `SystemTime`, no wall-clock reads.** All randomness draws from the room's seeded `rand_pcg::Pcg64`. If you need a new random value somewhere, thread the RNG through — don't reach for a global.
- **No `HashMap` / `HashSet` iteration in simulation logic.** Use `BTreeMap` / `BTreeSet`, or collect to a `Vec` and `sort_by_key`. Hash iteration order is randomized per-process and will desync replays.
- **Commands are applied in `bot_id` order, never in arrival order.** If you touch the tick loop, preserve the sort.
- **Pin floats to `f32` consistently.** Don't mix `f64` partway through a calculation. The `glam` crate's `Vec2` (which is `f32`) is the standard.
- **Physics step is fixed `dt = 0.1s`.** The wall clock is only used to *pace* the loop (sleep until next tick), never to *drive* the physics.
- **Every mid-match world mutation is a replay input.** Anything that changes the world outside a bot command — a disconnect, an operator kick, a roster change — must be written to the replay log (extend `ReplayRecord`, bump the format version) and re-applied at the same tick during replay. If live and replay can diverge on it, it belongs in the log.
- **Pending commands are consumed unconditionally at the top of the next `step_tick()`.** The `tick` field on a queued command does not gate consumption. Replay injection must happen exactly one step before the recorded tick — never "inject, then step N times to catch up".

When in doubt: if the code path runs inside `step_tick()`, assume it must be deterministic.

Powerups (`server/src/sim/powerups.rs`) are part of this contract: every effect helper reads only `Ship`, `World`, `world.tick`, and `PowerupConfig` — never wall-clock or `thread_rng`. The catalog of one-off powerups bots pick at match start lives there; see `docs/POWERUPS.md` for the published behaviour.

### Trust boundary

The server never executes bot code. Bots are remote WebSocket clients. Anything coming in on `/bot` is **untrusted input** — validate it, bound it, and never let a malformed message panic the server task. A bot sending garbage should get an `error` message and possibly a disconnect, never crash the room.

### Sensor filtering is the bot's only view

Bots receive a *filtered* `tick` message computed from their sensor mode. They must never receive ground-truth state. If you're tempted to add a field to the bot's `tick` payload "just for debugging," put it behind a server flag (`--debug-bot-omniscience`) that's off by default — and never on in tournament mode.

This applies to **events**, not just contacts. Anything in the bot-facing `events` array must be anonymized exactly like contacts (no persistent ground-truth `ShipId`s — contacts are re-anonymized per tick for a reason) and gated by the *actual* sensor result for that viewer and tick. Do not write a parallel "is it visible" reimplementation next to `sensors.rs`; it will drift.

Spectators get full ground truth. Don't conflate the two payloads.

---

## Repository layout

```
server/         Rust binary. Cargo workspace root is here.
  src/main.rs       CLI + runtime startup
  src/net.rs        axum HTTP/WebSocket front end: REST control plane + /bot + /spectate
  src/room.rs       Room state machine (lobby → running → ended)
  src/auth.rs       Admin password + JWT issue/verify
  src/sim/          Deterministic simulation — handle with care
  src/sim/config.rs Per-match balance parameters (SimConfig)
  src/protocol.rs   serde types for the wire protocol
  src/replay.rs     JSONL replay log

sdk-python/     Reference Python SDK
spectator/      Svelte + TypeScript + Vite app, built to spectator/dist/ and
                baked into the server binary via `include_str!`. Served at /.
                Pure logic lives under src/lib/ (unit-tested with Vitest);
                Svelte components in src/components/ are thin glue.
examples/       Example bots (circle_bot.py, powerful_bot.py, tracking_bot.py,
                tactician_bot.py, strategist_bot.py, loadout_bot.py)
system-design.md  Full design doc — source of truth for architecture (repo root)
docs/
  PROTOCOL.md        Wire protocol spec, kept in sync with src/protocol.rs
  POWERUPS.md        Published powerup catalog and behaviour
  REVIEW-FINDINGS.md Prioritized open findings from the 2026-07 review — check it
                     before starting work in an area; your bug may already be filed
                     with a fix sketch and acceptance criteria.
```

When you change the wire protocol, update **all four** of: `server/src/protocol.rs`, `docs/PROTOCOL.md`, the SDK (`sdk-python/naval_sdk/protocol.py`), and the spectator types (`spectator/src/types/protocol.ts`). The protocol doc is the public contract; if it drifts from the code, players' bots break silently — and stale spectator types silently drop fields in the viewer.

---

## Common commands

```bash
# Server
cd server
cargo run -- --port 7878 --tick-hz 10 --seed 42         # Run a default room
cargo test                                              # Unit + replay tests
cargo clippy --all-targets -- -D warnings               # Lint (CI runs this)
cargo fmt                                               # Format

# Replay an existing match
cargo run -- --replay ./replays/match_20260508_171203.jsonl

# Python SDK
cd sdk-python
pip install -e .
pytest

# Run an example bot against a local server
python examples/tactician_bot.py --host localhost --port 7878 --name tactician

# Spectator (Svelte / TS / Vite)
cd spectator
npm install                  # first time only
npm run build                # emits dist/{index.html,index.js,index.css}
                             #   — these are include_str!'d by the server, so
                             #   you must rebuild the server crate after.
npm test                     # vitest run — lib/ unit tests
npm run dev                  # http://localhost:5173 with HMR.
                             #   /spectate + /bot are proxied to ws://localhost:7878,
                             #   so run `cargo run` in another terminal first.

# Once the spectator is built, visit http://localhost:7878/ to view a match
# via the server's static handler.

# Containerised run (single command, no local Rust/Node needed)
docker compose up --build    # builds the multi-stage image and serves on :7878.
                             #   Replays land in ./replays/ via bind-mount.

# Start every example bot, each in its own container.
# Needs a server already running (the `docker compose up` above, or `cargo run`).
docker compose -f docker-compose.bots.yml up --build
                             #   Bots reach the server at host.docker.internal:7878.
                             #   Override with SERVER_HOST / SERVER_PORT env vars.
```

The room is driven over a REST control plane (`/api/*`), not stdin — there is no operator command interface. Lifecycle actions (`start`, `abort`, `reset`, `kick`) and parameter changes (`PUT /api/room/config`) are HTTP routes gated by a JWT. Get a token from `POST /api/login` with the admin password (`--admin-password` / `BATTLE_ADMIN_PASSWORD`, random per start if unset and logged once at INFO). The spectator web UI uses these routes to manage matches from the browser. See `docs/PROTOCOL.md §2.5`.

---

## Conventions

### Rust

- `cargo fmt` and `cargo clippy -D warnings` are required before commits. CI enforces both.
- Prefer `?` over `unwrap()` in non-test code. The only acceptable `unwrap()` in `sim/` is on invariants the type system can't express, with a comment explaining why.
- Async code uses `tokio`. Don't mix in `async-std` or `smol`.
- Logging: `tracing` with structured fields (`tracing::info!(bot_id = %id, "connected")`), not `println!`.
- Module boundary: `sim/` should not import from `net.rs` or `protocol.rs` directly. The room translates protocol messages into sim commands and back. This keeps the simulation testable without a network.
- **Room modes are a matrix, not a line.** The room's effective state is `state` × mode flags (`mc_run`, and any future mode). Every operator event handler (`start`, `abort`, `reset`, `kick`) must behave sensibly in *every* cell of that matrix, and every transition back to `Lobby` must clear (or `debug_assert!` empty) all mode state. If you add a mode, audit each `handle_event` arm before merging.
- **Don't publish dead tunables.** Every field exposed in `welcome.ship_specs` or the config schema must actually be read by the sim. Bot authors build strategy around published specs; operators assume tuning a knob does something.

### Python SDK

- Type hints required on the public API. Internal helpers can skip them.
- The SDK never panics on a malformed server message — it logs and continues. Bot authors will hit edge cases we didn't anticipate. Concretely: **every** field extraction *and conversion* from a server frame happens inside the message handler's `try` block. A conversion at the callback call site (e.g. indexing into a bound-but-unvalidated value) escapes the guard — that exact pattern has shipped a crash before.
- **Server ticks reset to 0 every match.** Any SDK or helper state keyed off an absolute tick (cooldowns, staleness windows, timers) must be reset in `on_game_start`. If a helper class holds such state, give it a `reset()` method and call it — don't reason your way out of it in a comment.
- **Match-scoped server state does not survive the lobby.** The server drops committed powerup loadouts (and `ready` flags) whenever the room returns to lobby. Anything the SDK sends "once" at `welcome` time but the server scopes per-match must be re-sent on every `lobby` message.
- `raw_send(dict)` and `raw_recv()` escape hatches stay public. Power users need to bypass the typed API sometimes.

### Spectator

- **Never hardcode server-tunable values in components or the renderer** (map size, radar range, ammo, speeds, HP). Thread them from real data — `welcome`/room config for the live view, the replay header for the replay view. The values in `src/lib/constants.ts` are last-resort fallbacks only: they must equal the server defaults in `server/src/sim/constants.rs`, and the two files change together.
- `spectator/dist/` is baked into the server binary via `include_str!`. Any change under `spectator/src/` must be accompanied by `npm run build` and the regenerated `dist/` in the same commit, then a server-crate rebuild.
- Async stores that fetch (perspectives, replays): clear the displayed data before starting a fetch, and tag in-flight requests so a stale resolution can't overwrite newer state.

### Protocol changes

The wire protocol is an external contract. When changing it:

1. Update `server/src/protocol.rs`.
2. Update `docs/PROTOCOL.md` to match — same field names, same examples.
3. Update the Python SDK's `protocol.py`.
4. Update the spectator's `src/types/protocol.ts` (spectator, replay, and REST payload types live there too — stale entries silently drop data in the viewer).
5. If the change is breaking, bump the version string sent in the `welcome` message and document the break in `docs/PROTOCOL.md` under a "Changelog" section.
6. Run the example bots in `examples/` against the new server. They serve as integration tests.

Additive changes (new optional field) are usually safe. Renames, type changes, and removed fields are breaking and need a version bump.

---

## Testing

- **Unit tests** live next to the code (`#[cfg(test)] mod tests` in Rust, `tests/` for Python).
- **Replay tests** are the single most valuable test category here. A replay test loads a recorded JSONL log, re-runs the simulation, and asserts the final world state matches the recorded final state byte-for-byte. If a refactor breaks determinism, replay tests catch it immediately. Add one whenever you fix a determinism bug.
- **Replay tests must include irregular bots.** A replay test whose bots command every tick and never disconnect proves nothing about the paths where replays actually break. When touching replay or the tick loop, include scenarios with: a bot that skips ticks between commands, a bot that goes silent for many ticks, and (once supported in the log format) a mid-match disconnect.
- **Test the failure, not just the exit.** A disconnect/violation test that accepts a transport error as success would also pass if the server crashed. Assert the specific error frame / close code the contract promises.
- **Integration test**: `server/tests/two_bot_match.rs` spins up the server in-process, connects two scripted bots, and runs a full match to completion. Slow but high-signal — keep it green.

Don't write tests that depend on wall-clock timing inside the simulation. If a test needs to "wait for a tick," step the simulation manually instead of sleeping.

---

## Things to ask before doing

These are choices that look like implementation details but have design implications. Surface them before going deep:

- **Adding a new field to the `tick` payload** — does it leak ground-truth info past the sensor filter?
- **Adding a new command verb** — does it affect determinism? Is it bounded (no infinite loops, no unbounded allocations)?
- **Changing physics constants** — these are balance decisions, not just numbers. Existing bots and replays depend on them.
- **Adding a dependency** — especially in `server/`. Keep the simulation crate's dependency tree small; pull heavy deps into the binary crate or a separate module.
- **"Just a quick `HashMap`"** in `sim/` — see determinism rules above. Use `BTreeMap` or sort.

---

## Things that are fine to do without asking

- Improving error messages, especially in protocol validation. Bot authors will thank you.
- Adding example bots in `examples/`. Variety helps onboarding.
- Polishing the spectator UI — it's the demo surface and any improvement is welcome.
- Adding logging at `debug!` or `trace!` level.
- Writing more replay tests.
- Fixing typos, broken links, stale doc snippets.

---

## Hackathon mode reminder

This is built for a hackathon, not production. Some deliberate omissions, listed so they're not "fixed" by accident:

- The admin REST plane (`/api/*`) is gated by a JWT, but the `/bot` and `/spectate` WebSocket endpoints are intentionally unauthenticated. No TLS, no rate limiting. Local play only.
- No persistence besides replay JSONL files. Rooms vanish when the server stops.
- No matchmaking. The operator starts the match via the REST API / web UI.
- One server process, one configurable room.

If you're tempted to add a database, TLS, or auth to the `/bot` / `/spectate` endpoints, stop and check whether the scope has actually changed. For the hackathon target, simpler is the goal.
