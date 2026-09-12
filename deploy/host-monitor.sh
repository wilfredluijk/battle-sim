#!/usr/bin/env bash
set -euo pipefail
curl --fail --silent --show-error --max-time 2 http://127.0.0.1:7878/healthz >/dev/null
/usr/local/sbin/battle-cert-check
python3 - <<'PY'
import shutil
assert shutil.disk_usage('/opt/battle-sim').free >= 2 * 1024**3, 'battle-sim disk space below 2 GiB'
PY
