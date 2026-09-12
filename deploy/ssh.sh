#!/usr/bin/env bash
set -euo pipefail
base=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
exec ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -o IdentitiesOnly=yes -o ForwardAgent=no -o StrictHostKeyChecking=yes -o "UserKnownHostsFile=$base/known_hosts" -i "${BATTLE_SSH_KEY:-$HOME/.ssh/id_ed25519}" root@93.190.187.250 "$@"
