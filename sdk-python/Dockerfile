# Container image for running a Python example bot against the naval-server.
#
# The entrypoint selects the bot script via $BOT_SCRIPT (path under /app/bots/)
# and waits for the server to accept TCP connections before launching, so the
# bot can be started concurrently with the server under docker-compose.

FROM python:3.12-slim-bookworm

RUN apt-get update && \
    apt-get install -y --no-install-recommends netcat-openbsd && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Install the SDK as a package so example bots can `from naval_sdk import ...`.
COPY sdk-python/ /app/sdk-python/
RUN pip install --no-cache-dir /app/sdk-python

# Example bots; selected at runtime via $BOT_SCRIPT.
COPY examples/python/ /app/bots/

COPY sdk-python/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV BOT_HOST=server \
    BOT_PORT=7878 \
    BOT_NAME=bot \
    BOT_SCRIPT=circle_bot.py \
    BOT_SEED=0 \
    BOT_WAIT_TIMEOUT=30

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
