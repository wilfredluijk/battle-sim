#!/usr/bin/env python3
"""Retain at most 200 completed replays / 1 GiB / 30 days; protect active files."""
from pathlib import Path
import time
root = Path('/opt/battle-sim/replays')
now = time.time()
completed = []
for path in root.glob('*.jsonl'):
    stat = path.stat()
    # Never touch a replay that might still be open in the current 5-minute match.
    if now - stat.st_mtime < 3600:
        continue
    with path.open('rb') as f:
        f.seek(max(0, stat.st_size - 4096))
        tail = f.read().splitlines()
    is_complete = bool(tail and b'"type":"end"' in tail[-1])
    if is_complete:
        completed.append((stat.st_mtime, stat.st_size, path))
    elif now - stat.st_mtime > 7 * 86400:
        path.unlink()
        path.with_suffix('.build.json').unlink(missing_ok=True)
completed.sort(reverse=True)
size = 0
for index, (mtime, length, path) in enumerate(completed):
    size += length
    if index >= 200 or size > 1024**3 or now - mtime > 30 * 86400:
        path.unlink()
        path.with_suffix('.build.json').unlink(missing_ok=True)
