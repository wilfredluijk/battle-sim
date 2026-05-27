# Naval Battle Simulator

A hackathon programming game. Players write bots in any language, connect them
to a central Rust server over WebSocket, and battle in a deterministic top-down
naval simulation. A browser spectator renders matches live, and every match is
saved as a JSONL replay log that can be re-played at full fidelity.

**Pick the doc you need:**
- **Writing a bot?** Start with the SDK guide: [`sdk-python/README.md`](sdk-python/README.md). Runnable examples live under [`examples/`](examples/).
- **Implementing the wire protocol directly?** Read [`docs/PROTOCOL.md`](docs/PROTOCOL.md) — every Bot↔Server and Server→Spectator frame is documented there.
- **Curious how the game works?** [`system-design.md`](system-design.md) covers physics, sensors, weapons, and the trust model.

## Repository layout

```
server/         Rust binary — authoritative simulation, WebSocket server, replay log
sdk-python/     Reference Python SDK (`pip install -e .`); README is the bot author's manual
examples/       Runnable example bots covering each tactical layer
spectator/      Svelte + TypeScript + Vite app. Bundle is built to spectator/dist/
                and baked into the server binary via `include_str!`.
docs/           PROTOCOL.md (wire protocol) + design-decisions/
Dockerfile      Multi-stage build (node → rust → debian-slim) producing a
                self-contained server image.
docker-compose.yml  One-service compose for `docker compose up --build`.
system-design.md  Architecture and gameplay reference
```

## Prerequisites

- **Rust** 1.86+ (stable). Install via [rustup](https://rustup.rs/).
- **Node** 20+ and **npm** for building the spectator (not needed if you use Docker).
- A WebSocket-capable browser for the spectator UI (any modern Chromium / Firefox / Safari).
- Optional: `wscat` (`npm i -g wscat`) for poking at the protocol by hand.

## Build and run the server

The server embeds the spectator bundle at compile time, so the spectator must be
built once before the first `cargo build`:

```bash
cd spectator
npm install        # one-time
npm run build      # emits dist/{index.html,index.js,index.css}

cd ../server
cargo run -- --port 7878 --tick-hz 10 --seed 42
```

That boots a single room called `main` listening on `127.0.0.1:7878`. The room
is driven entirely over the REST control plane (`/api/*`); the spectator web UI
at `http://localhost:7878/` is the easy way to log in, set match parameters, and
start the match once every connected bot has signaled `ready`.

The server prints a one-time admin password at startup (look for the
`admin password` log line). Pin a fixed one with `--admin-password` or the
`BATTLE_ADMIN_PASSWORD` env var. Hit Ctrl-C to shut down cleanly.

### Or use Docker

If you don't want to install Rust + Node locally, the compose file builds the
whole stack (spectator → server) in a multi-stage image:

```bash
docker compose up --build
```

The server listens on `127.0.0.1:7878` — open the spectator UI there to drive
the room. Replays land in `./replays/` via a bind-mount. The admin password is
printed in the container logs (`docker compose logs`); pin it by setting
`BATTLE_ADMIN_PASSWORD` in the compose file's `environment`. Stop with
`docker compose down`.

### Spectator dev loop

For iterating on the spectator UI without rebuilding the Rust binary each time:

```bash
# terminal 1
cd server
cargo run -- --port 7878

# terminal 2
cd spectator
npm run dev     # http://localhost:5173, HMR enabled, /spectate proxied to 7878
```

Run `npm test` for the Vitest unit tests against `src/lib/`.

### Server flags

| Flag | Default | Purpose |
|---|---|---|
| `--port` | `7878` | TCP port for the WebSocket / HTTP listener |
| `--tick-hz` | `10` | Simulation tick rate |
| `--tick-deadline-ms` | `80` | Per-tick window for bots to submit commands |
| `--map` | `700x700` | Map size in `WIDTHxHEIGHT` units |
| `--max-bots` | `24` | Maximum bots per room |
| `--seed` | `42` | RNG seed (drives all simulation randomness) |
| `--replay-dir` | `./replays` | Where match replay JSONL files are written |
| `--replay <FILE>` | — | Replay a saved match instead of accepting bot connections |
| `--admin-password` | random | Password for `POST /api/login` (logged once at startup if unset; also `BATTLE_ADMIN_PASSWORD`) |
| `--token-ttl-hours` | `12` | Lifetime of issued admin JWTs |

`cargo run -- --help` prints the same list.

## Watch a match

With the server running, open

```
http://localhost:7878/
```

The spectator UI loads automatically. Before a match it shows the pre-match
lobby — connected bots and the editable match parameters; during a match, the
live battlefield with a sidebar of tick / players / events; afterwards, a match
report. Log in with the admin password to edit parameters and control the room.
Active-radar pings show as faint translucent rings.

## Connect a bot

The server speaks JSON over WebSocket at `ws://localhost:7878/bot`. For a
quick smoke test with `wscat`:

```bash
wscat -c ws://localhost:7878/bot
> {"type":"hello","name":"manual_bot","version":"1.0"}
< {"type":"welcome","bot_id":"b_1",...}
> {"type":"ready"}
```

Then start the match from the spectator UI's pre-match lobby (log in with the
admin password first). The bot will start receiving `tick` frames; reply with
`command` messages each tick. Full message reference in
[`docs/PROTOCOL.md`](docs/PROTOCOL.md).

In practice you'll use the reference Python SDK ([`sdk-python/`](sdk-python/))
and start from an example under [`examples/`](examples/) (`circle_bot.py`,
`tactician_bot.py`, `strategist_bot.py`, …). The SDK owns the WebSocket, the
handshake, and frame dispatch; you only override `on_tick`. The SDK README
covers the API surface, match lifecycle, and the optional tactical toolkit
(helm / tracker / sensor / evader helpers).

## Replays

Every match leaves a JSONL log in `--replay-dir` (default `./replays/`):

```
replays/match_main_1715000000.jsonl
```

To re-play it visually, start the server in replay mode and open the spectator
page as usual:

```bash
cd server
cargo run -- --port 7878 --replay ./replays/match_main_1715000000.jsonl
```

The simulation re-runs at the recorded tick rate and broadcasts to spectators;
no bot connections are accepted while in replay mode. Replays are
byte-identical to the original run — that's enforced by
`server/tests/replay_determinism.rs`.

## Testing

```bash
cd server
cargo test                                  # unit + integration + replay tests
cargo clippy --all-targets -- -D warnings   # lint gate
cargo fmt                                   # format

cd ../sdk-python
pytest                                      # Python SDK unit tests
```

The same gates run in CI (see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)):
`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, and `pytest` must
all be green.
