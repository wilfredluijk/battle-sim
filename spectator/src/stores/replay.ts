// Replay viewer state. The whole timeline is fetched up front, so playback, seeking and
// scrubbing are pure client-side operations over an in-memory array — no server round
// trips and no involvement of the live `/spectate` connection.

import { get, writable } from 'svelte/store';
import { fetchPerspective, fetchReplay } from '../lib/replayApi';
import { extractMarkers, type ReplayMarker } from '../lib/replayMarkers';
import type { CapturedPerspective, CapturedReplay } from '../types/protocol';
import { appMode } from './index';

/** Perspective selector value: ground truth, or a specific bot's sensor view. */
export type Perspective = 'overall' | string;

export const replayData = writable<CapturedReplay | null>(null);
export const replayMarkers = writable<ReplayMarker[]>([]);
export const replayTick = writable<number>(0);
export const replayPlaying = writable<boolean>(false);
export const replaySpeed = writable<number>(1);
export const replayPerspective = writable<Perspective>('overall');
/** Captured frames for the selected bot perspective; `null` while in `overall` mode. */
export const replayPerspectiveData = writable<CapturedPerspective | null>(null);
export const replayLoading = writable<boolean>(false);
export const replayError = writable<string | null>(null);

// Perspective re-runs are cached so flipping back to a bot is instant.
const perspectiveCache = new Map<string, CapturedPerspective>();
let currentReplayId: string | null = null;

/** Highest tick index in the loaded timeline (`0` when nothing is loaded). */
export function finalTick(): number {
  const d = get(replayData);
  return d ? d.frames.length - 1 : 0;
}

/** Fetch a replay's full timeline and open the viewer, paused at tick 0. */
export async function openReplay(id: string): Promise<void> {
  replayLoading.set(true);
  replayError.set(null);
  try {
    const data = await fetchReplay(id);
    currentReplayId = id;
    perspectiveCache.clear();
    replayData.set(data);
    replayMarkers.set(extractMarkers(data.frames));
    replayTick.set(0);
    replayPlaying.set(false);
    replaySpeed.set(1);
    replayPerspective.set('overall');
    replayPerspectiveData.set(null);
    appMode.set('replay-viewer');
  } catch (e) {
    replayError.set(e instanceof Error ? e.message : 'failed to load replay');
  } finally {
    replayLoading.set(false);
  }
}

/** Switch the rendered perspective, lazily fetching a bot's sensor timeline on first use. */
export async function selectPerspective(p: Perspective): Promise<void> {
  replayPerspective.set(p);
  if (p === 'overall') {
    replayPerspectiveData.set(null);
    return;
  }
  const cached = perspectiveCache.get(p);
  if (cached) {
    replayPerspectiveData.set(cached);
    return;
  }
  if (!currentReplayId) return;
  replayLoading.set(true);
  replayError.set(null);
  try {
    const data = await fetchPerspective(currentReplayId, p);
    perspectiveCache.set(p, data);
    // The user may have changed the selector while the request was in flight.
    if (get(replayPerspective) === p) replayPerspectiveData.set(data);
  } catch (e) {
    replayError.set(e instanceof Error ? e.message : 'failed to load perspective');
    replayPerspective.set('overall');
    replayPerspectiveData.set(null);
  } finally {
    replayLoading.set(false);
  }
}

/** Jump to `tick`, clamped to the timeline bounds. */
export function seekTo(tick: number): void {
  const max = finalTick();
  replayTick.set(Math.max(0, Math.min(max, Math.round(tick))));
}

/** Nudge the playhead by `delta` ticks. */
export function stepFrame(delta: number): void {
  seekTo(get(replayTick) + delta);
}

/** Toggle play/pause. Pressing play at the end restarts from tick 0. */
export function togglePlay(): void {
  if (!get(replayData)) return;
  const playing = get(replayPlaying);
  if (!playing && get(replayTick) >= finalTick()) replayTick.set(0);
  replayPlaying.set(!playing);
}

/** Advance one tick during playback; pause automatically at the end. Driven by the
 * viewer's interval ticker. */
export function advanceTick(): void {
  const max = finalTick();
  const cur = get(replayTick);
  if (cur >= max) {
    replayPlaying.set(false);
    return;
  }
  replayTick.set(cur + 1);
}

/** Leave the viewer and return to the live spectator screen. */
export function exitReplay(): void {
  replayPlaying.set(false);
  replayData.set(null);
  replayMarkers.set([]);
  replayPerspectiveData.set(null);
  replayError.set(null);
  perspectiveCache.clear();
  currentReplayId = null;
  appMode.set('live');
}
