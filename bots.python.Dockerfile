# Image for the Python example bots (examples/*.py).
# Built once and shared by every Python bot service in docker-compose.bots.yml.
#
# Uses a per-Dockerfile ignore file (bots.python.Dockerfile.dockerignore) so the
# build context keeps sdk-python/ and examples/ — the root .dockerignore drops them.
FROM python:3.12-slim

WORKDIR /app

# Install the SDK (this also pulls its `websockets` dependency). Copying the SDK
# on its own layer means a bot-only edit does not trigger a reinstall.
COPY sdk-python/ ./sdk-python/
RUN pip install --no-cache-dir ./sdk-python

# The examples accept --env-file for a mounted participant file, or inherit
# BATTLE_SERVER_URL / BATTLE_BOT_TOKEN from the container environment.
COPY examples/ ./bots/

ENTRYPOINT ["python"]
