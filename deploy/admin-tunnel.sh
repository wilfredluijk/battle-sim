#!/usr/bin/env bash
set -euo pipefail
base=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
echo 'Admin UI: http://127.0.0.1:8787 (keep this terminal open)'
exec ssh -F /dev/null -o BatchMode=yes -o IdentitiesOnly=yes -o ForwardAgent=no -o StrictHostKeyChecking=yes -o "UserKnownHostsFile=$base/known_hosts" -i "${BATTLE_SSH_KEY:-$HOME/.ssh/id_ed25519}" -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 -N -L 127.0.0.1:8787:127.0.0.1:7878 root@93.190.187.250
