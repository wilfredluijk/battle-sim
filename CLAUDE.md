# CLAUDE.md

Guidance for Claude when working in this repository. Read this before making non-trivial changes.

---

## What this project is

A hackathon programming game. Players write bots in any language, connect them to a central Rust server over WebSocket, and battle in a deterministic top-down naval simulation. A browser spectator renders matches live. The full system design lives in `docs/system-design.md` — read it first if you're new to the repo.

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

When in doubt: if the code path runs inside `step_tick()`, assume it must be deterministic.

### Trust boundary

The server never executes bot code. Bots are remote WebSocket clients. Anything coming in on `/bot` is **untrusted input** — validate it, bound it, and never let a malformed message panic the server task. A bot sending garbage should get an `error` message and possibly a disconnect, never crash the room.

### Sensor filtering is the bot's only view

Bots receive a *filtered* `tick` message computed from their sensor mode. They must never receive ground-truth state. If you're tempted to add a field to the bot's `tick` payload "just for debugging," put it behind a server flag (`--debug-bot-omniscience`) that's off by default — and never on in tournament mode.

Spectators get full ground truth. Don't conflate the two payloads.

---

## Repository layout

```
server/         Rust binary. Cargo workspace root is here.
  src/main.rs       CLI + runtime startup
  src/net.rs        WebSocket accept, connection tasks
  src/room.rs       Room state machine (lobby → running → ended)
  src/sim/          Deterministic simulation — handle with care
  src/protocol.rs   serde types for the wire protocol
  src/replay.rs     JSONL replay log
  src/control.rs    stdin command parser

sdk-python/     Reference Python SDK
spectator/      Static HTML/JS, served by the Rust server at /
examples/       Example bots (circle_bot.py, chaser_bot.py, sniper_bot.py)
docs/
  system-design.md   Full design doc — source of truth for architecture
  PROTOCOL.md        Wire protocol spec, kept in sync with src/protocol.rs
  QUICKSTART.md      5-minute onboarding for new players
```

When you change the wire protocol, update **all three** of: `server/src/protocol.rs`, `docs/PROTOCOL.md`, and the SDK. The protocol doc is the public contract; if it drifts from the code, players' bots break silently.

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
python examples/chaser_bot.py --host localhost --port 7878 --name chaser

# Spectator: just open spectator/index.html, or visit http://localhost:7878/
```

The server reads operator commands from stdin while running. Type `room list`, `room start main`, `quit`, etc. See `docs/system-design.md §3.3` for the full list.

---

## Conventions

### Rust

- `cargo fmt` and `cargo clippy -D warnings` are required before commits. CI enforces both.
- Prefer `?` over `unwrap()` in non-test code. The only acceptable `unwrap()` in `sim/` is on invariants the type system can't express, with a comment explaining why.
- Async code uses `tokio`. Don't mix in `async-std` or `smol`.
- Logging: `tracing` with structured fields (`tracing::info!(bot_id = %id, "connected")`), not `println!`.
- Module boundary: `sim/` should not import from `net.rs` or `protocol.rs` directly. The room translates protocol messages into sim commands and back. This keeps the simulation testable without a network.

### Python SDK

- Type hints required on the public API. Internal helpers can skip them.
- The SDK never panics on a malformed server message — it logs and continues. Bot authors will hit edge cases we didn't anticipate.
- `raw_send(dict)` and `raw_recv()` escape hatches stay public. Power users need to bypass the typed API sometimes.

### Protocol changes

The wire protocol is an external contract. When changing it:

1. Update `server/src/protocol.rs`.
2. Update `docs/PROTOCOL.md` to match — same field names, same examples.
3. Update the Python SDK's `protocol.py`.
4. If the change is breaking, bump the version string sent in the `welcome` message and document the break in `docs/PROTOCOL.md` under a "Changelog" section.
5. Run the example bots in `examples/` against the new server. They serve as integration tests.

Additive changes (new optional field) are usually safe. Renames, type changes, and removed fields are breaking and need a version bump.

---

## Testing

- **Unit tests** live next to the code (`#[cfg(test)] mod tests` in Rust, `tests/` for Python).
- **Replay tests** are the single most valuable test category here. A replay test loads a recorded JSONL log, re-runs the simulation, and asserts the final world state matches the recorded final state byte-for-byte. If a refactor breaks determinism, replay tests catch it immediately. Add one whenever you fix a determinism bug.
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

- No auth, no TLS, no rate limiting on the WebSocket endpoints. Local play only.
- No persistence besides replay JSONL files. Rooms vanish when the server stops.
- No matchmaking. The operator starts rooms manually via stdin.
- One server process, ideally one room at a time. Multi-room is supported but untested at scale.

If you're tempted to add an auth layer or a database, stop and check whether the scope has actually changed. For the hackathon target, simpler is the goal.
