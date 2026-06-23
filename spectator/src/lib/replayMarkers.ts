// Timeline event extraction for the replay slider. Pure functions over the ground-truth
// frames so they are cheap to unit-test and run once per loaded replay.

import type { WorldFrame } from '../types/protocol';

export type ReplayMarkerKind = 'fired' | 'hit' | 'kill' | 'powerup';

/** A point of interest on the replay timeline, placed at `tick`. */
export interface ReplayMarker {
  tick: number;
  kind: ReplayMarkerKind;
}

/**
 * Scan a replay's ground-truth frames for slider markers:
 *
 * - `hit` / `kill` / `powerup` come straight from each frame's `events`.
 * - `fired` is derived: a shell `id_index` that was absent the previous frame means a
 *   shot was loosed that tick. The wire protocol has no explicit "fired" event, so this
 *   diff is how the viewer surfaces one without a protocol change.
 *
 * At most one marker of each kind is emitted per tick — the slider only needs to know a
 * tick is interesting, not how many times.
 */
export function extractMarkers(frames: WorldFrame[]): ReplayMarker[] {
  const markers: ReplayMarker[] = [];
  let prevShells = new Set<number>();

  for (const frame of frames) {
    const shells = new Set<number>();
    for (const s of frame.shells) shells.add(s.id_index);

    let fired = false;
    for (const id of shells) {
      if (!prevShells.has(id)) {
        fired = true;
        break;
      }
    }
    if (fired) markers.push({ tick: frame.tick, kind: 'fired' });

    let hit = false;
    let kill = false;
    let powerup = false;
    for (const ev of frame.events) {
      if (ev.type === 'hit') hit = true;
      else if (ev.type === 'death') kill = true;
      else if (ev.type === 'powerup_activated') powerup = true;
    }
    if (hit) markers.push({ tick: frame.tick, kind: 'hit' });
    if (kill) markers.push({ tick: frame.tick, kind: 'kill' });
    if (powerup) markers.push({ tick: frame.tick, kind: 'powerup' });

    prevShells = shells;
  }

  return markers;
}
