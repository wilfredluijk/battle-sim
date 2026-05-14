#!/usr/bin/env bash
# Integration test harness.
#
# For each scenario directory under ./scenarios/:
#   1. docker compose up the stack with --abort-on-container-exit
#   2. wait for the server container to exit on its own (room ends via
#      --max-ticks or last-bot-standing)
#   3. find the JSONL replay the server wrote to the mounted volume
#   4. run `cargo run --bin validate-replay` against it with the scenario's
#      expect.toml
#   5. tear the stack down
#
# Exits non-zero on the first failure. Usage:
#   ./run.sh                  # run every scenario
#   ./run.sh <name> [<name>]  # run only the named scenarios

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SCENARIOS_DIR="$SCRIPT_DIR/scenarios"

if ! command -v docker >/dev/null 2>&1; then
    echo "harness: docker is required but not installed" >&2
    exit 2
fi
if ! docker compose version >/dev/null 2>&1; then
    echo "harness: 'docker compose' v2 plugin is required" >&2
    exit 2
fi

# Build the validator once up front. The harness re-invokes it per scenario; an
# incremental rebuild is essentially free, but the first compile shouldn't sit
# inside the per-scenario timing budget.
echo "harness: building validate-replay…"
(cd "$REPO_ROOT/server" && cargo build --bin validate-replay --quiet)
VALIDATOR="$REPO_ROOT/server/target/debug/validate-replay"

run_scenario() {
    local name="$1"
    local dir="$SCENARIOS_DIR/$name"
    if [[ ! -d "$dir" ]]; then
        echo "harness: scenario `$name` not found at $dir" >&2
        return 2
    fi
    local compose="$dir/docker-compose.yml"
    local expect="$dir/expect.toml"
    if [[ ! -f "$compose" || ! -f "$expect" ]]; then
        echo "harness: scenario `$name` missing docker-compose.yml or expect.toml" >&2
        return 2
    fi

    # Fresh replays dir per run so the validator picks up exactly the file this
    # run produced, not a stale leftover.
    local replays_dir="$dir/replays"
    rm -rf "$replays_dir"
    mkdir -p "$replays_dir"

    echo
    echo "=== scenario: $name ==="
    # `--abort-on-container-exit` brings the whole stack down the moment any
    # container exits. The server exits cleanly when its room ends; bots may
    # exit slightly later when their WS disconnects.
    (cd "$dir" && docker compose -p "battlesim-int-$name" up \
        --build \
        --abort-on-container-exit \
        --exit-code-from server)
    local rc=$?

    # Always tear down so a failure doesn't leave containers/volumes around.
    (cd "$dir" && docker compose -p "battlesim-int-$name" down --volumes --remove-orphans >/dev/null 2>&1 || true)

    if [[ $rc -ne 0 ]]; then
        echo "harness: scenario `$name` server exited with $rc" >&2
        return 1
    fi

    # Pick the newest .jsonl in the replays mount — the server may have written
    # other logs in previous runs that didn't get cleared, but we wipe on entry,
    # so there should be exactly one.
    local replay
    replay=$(ls -t "$replays_dir"/*.jsonl 2>/dev/null | head -n 1 || true)
    if [[ -z "${replay:-}" ]]; then
        echo "harness: scenario `$name` produced no replay file in $replays_dir" >&2
        return 1
    fi

    if ! "$VALIDATOR" "$replay" --expect "$expect"; then
        return 1
    fi
}

declare -a scenarios
if [[ $# -gt 0 ]]; then
    scenarios=("$@")
else
    mapfile -t scenarios < <(find "$SCENARIOS_DIR" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)
fi

if [[ ${#scenarios[@]} -eq 0 ]]; then
    echo "harness: no scenarios found under $SCENARIOS_DIR" >&2
    exit 2
fi

failed=()
for name in "${scenarios[@]}"; do
    if ! run_scenario "$name"; then
        failed+=("$name")
    fi
done

echo
if [[ ${#failed[@]} -eq 0 ]]; then
    echo "harness: all ${#scenarios[@]} scenario(s) passed"
    exit 0
else
    echo "harness: ${#failed[@]} scenario(s) failed: ${failed[*]}" >&2
    exit 1
fi
