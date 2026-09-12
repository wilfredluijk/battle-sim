#!/usr/bin/env bash
set -euo pipefail
release=${1:?release name required}
[[ "$release" =~ ^r[0-9]{8}-[0-9]+$ ]] || exit 2
cd "/opt/battle-sim/releases/$release"
sha256sum -c battle-sim.tar.sha256
docker image load -i battle-sim.tar
expected=$(cat image-id.txt)
actual=$(docker image inspect "battle-sim:$release" --format '{{.Id}}')
test "$expected" = "$actual"
if test -L /opt/battle-sim/current; then
    previous=$(readlink -f /opt/battle-sim/current)
    if test "$previous" != "$PWD"; then ln -sfn "$previous" /opt/battle-sim/previous; fi
fi
docker compose --env-file .env -f compose.prod.yml up -d --no-build --pull never --wait --wait-timeout 45
ln -sfn "$PWD" /opt/battle-sim/current
docker inspect battle-sim --format '{{json .State.Health}}'
