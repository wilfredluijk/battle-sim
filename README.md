# Naval Battle Simulator

A hackathon programming game. Players write bots in any language, connect them
to a central Rust server over WebSocket, and battle in a deterministic top-down
naval simulation. A browser spectator renders matches live, and every match is
saved as a JSONL replay log that can be re-played at full fidelity.

The full design lives in [`system-design.md`](system-design.md). The wire
protocol that bots speak lives in [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

## Repository layout

```
server/         Rust binary — authoritative simulation, WebSocket server, replay log
spectator/      Static HTML/JS canvas renderer (served by the Rust server at /)
docs/           PROTOCOL.md and friends
system-design.md  Architecture and gameplay reference
```

## Prerequisites

- **Rust** 1.75+ (stable). Install via [rustup](https://rustup.rs/).
- A WebSocket-capable browser for the spectator UI (any modern Chromium / Firefox / Safari).
- Optional: `wscat` (`npm i -g wscat`) for poking at the protocol by hand.

## Build and run the server

```bash
cd server
cargo run -- --port 7878 --tick-hz 10 --seed 42
```

That boots a single room called `main` listening on `127.0.0.1:7878`. The
server reads operator commands from stdin while running — type `help` for the
list. The most useful one is `room start main`, which transitions the room
from `Lobby` to `Running` once every connected bot has signaled `ready`.

Type `quit` (or hit Ctrl-C) to shut down cleanly.

### Server flags

| Flag | Default | Purpose |
|---|---|---|
| `--port` | `7878` | TCP port for the WebSocket / HTTP listener |
| `--tick-hz` | `10` | Simulation tick rate |
| `--tick-deadline-ms` | `80` | Per-tick window for bots to submit commands |
| `--map` | `1000x1000` | Map size in `WIDTHxHEIGHT` units |
| `--max-bots` | `4` | Maximum bots per room |
| `--seed` | `42` | RNG seed (drives all simulation randomness) |
| `--replay-dir` | `./replays` | Where match replay JSONL files are written |
| `--replay <FILE>` | — | Replay a saved match instead of accepting bot connections |

`cargo run -- --help` prints the same list.

## Watch a match

With the server running, open

```
http://localhost:7878/
```

The spectator UI loads automatically — ships, shells, splashes, and a sidebar
with tick / players / events. Active-radar pings show as faint translucent
rings.

## Connect a bot

The server speaks JSON over WebSocket at `ws://localhost:7878/bot`. For a
quick smoke test with `wscat`:

```bash
wscat -c ws://localhost:7878/bot
> {"type":"hello","name":"manual_bot","version":"1.0"}
< {"type":"welcome","bot_id":"b_1",...}
> {"type":"ready"}
```

Then in the server terminal: `room start main`. The bot will start receiving
`tick` frames; reply with `command` messages each tick. Full message reference
in [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

A reference Python SDK and example bots are planned (Phases 9 / 10 of
[`projectplan.md`](projectplan.md)) but not yet shipped.

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
```

CI expects `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test` to
all be green.
