#!/usr/bin/env sh
# Wait for the server's WebSocket port to open, then start the configured bot.
# Compose brings the server and bots up concurrently; the server needs a few
# hundred milliseconds to bind, so block until the port is reachable rather
# than racing it.
set -eu

deadline=$(( $(date +%s) + BOT_WAIT_TIMEOUT ))
while ! nc -z "$BOT_HOST" "$BOT_PORT" 2>/dev/null; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
        echo "bot: timeout waiting for $BOT_HOST:$BOT_PORT" >&2
        exit 1
    fi
    sleep 0.2
done

# --seed is only meaningful for bots whose RNG it actually drives (circle_bot).
# Pass it conditionally so bots without that flag aren't broken by an unknown arg.
if [ -n "${BOT_SEED:-}" ] && python "/app/bots/$BOT_SCRIPT" --help 2>/dev/null | grep -q -- '--seed'; then
    exec python "/app/bots/$BOT_SCRIPT" \
        --host "$BOT_HOST" \
        --port "$BOT_PORT" \
        --name "$BOT_NAME" \
        --seed "$BOT_SEED"
fi

exec python "/app/bots/$BOT_SCRIPT" \
    --host "$BOT_HOST" \
    --port "$BOT_PORT" \
    --name "$BOT_NAME"
