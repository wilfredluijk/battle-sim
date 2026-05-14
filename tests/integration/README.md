# Integration tests

End-to-end scenarios that exercise the **real** stack: the Rust server, the Python
SDK, and the wire protocol — all in containers, with no in-process shortcuts.
Validation runs against the recorded replay JSONL, so the test sees exactly what
a bot author would see.

The fast in-process tests under `server/tests/` (replay determinism, tick loop,
handshake) stay as the per-commit guardrail. The container-based suite lives
here and is a separate, slower lane — run it on demand.

## How a scenario is structured

```
scenarios/<name>/
  docker-compose.yml   # server + N bots, fixed seed, --max-ticks, --auto-start
  expect.toml          # assertions the validator applies to the replay
```

The compose file mounts a shared `./replays` directory; the server writes its
JSONL there, and the harness picks it up after the stack exits.

## Running

```bash
# Build images once (cached for subsequent runs).
docker compose -f scenarios/circle_vs_circle/docker-compose.yml build

# Run a single scenario.
./run.sh circle_vs_circle

# Run all scenarios.
./run.sh
```

`run.sh` brings the compose stack up with `--abort-on-container-exit`, waits for
the server to exit (the room shuts itself down when `--max-ticks` triggers or
the last ship dies), locates the newest replay in the mounted volume, and shells
out to `cargo run --bin validate-replay`. Non-zero exit codes from the validator
fail the run.

## Determinism

Every scenario pins `--seed` on the server and on any bot whose RNG accepts one.
Two runs of the same scenario must produce byte-identical replay logs — that's
the contract the validator depends on. If a scenario's outcome drifts, suspect
a non-determinism regression in the simulation before tweaking `expect.toml`.

## Adding a scenario

1. Copy an existing scenario directory.
2. Edit `docker-compose.yml`: change bot scripts, names, seeds, and the
   server's `--max-ticks` / `--max-bots`.
3. Run it once locally, inspect the produced replay (`docker compose run --rm
   server cat /app/replays/<id>.jsonl` or open it in the spectator), and write
   `expect.toml` against the observed outcome.
4. Commit both files.
