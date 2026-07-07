import { beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import { get } from 'svelte/store';
import type { CapturedPerspective, CapturedReplay } from '../types/protocol';

// Mock the REST layer so we control exactly when each fetch resolves.
vi.mock('../lib/replayApi', () => ({
  fetchReplay: vi.fn(),
  fetchPerspective: vi.fn(),
}));

import * as replayApi from '../lib/replayApi';
import {
  openReplay,
  selectPerspective,
  replayPerspective,
  replayPerspectiveData,
} from '../stores/replay';

const fetchReplay = replayApi.fetchReplay as Mock;
const fetchPerspective = replayApi.fetchPerspective as Mock;

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function fakeReplay(): CapturedReplay {
  return { header: { bots: [] }, frames: [], end: null } as unknown as CapturedReplay;
}

function fakePerspective(botId: string): CapturedPerspective {
  return { bot_id: botId, frames: [] };
}

describe('replay perspective races', () => {
  beforeEach(() => {
    fetchReplay.mockReset();
    fetchPerspective.mockReset();
    fetchReplay.mockResolvedValue(fakeReplay());
  });

  it('clears the old bot data while the newly selected bot timeline loads', async () => {
    await openReplay('A');

    fetchPerspective.mockResolvedValueOnce(fakePerspective('b_1'));
    await selectPerspective('b_1');
    expect(get(replayPerspectiveData)?.bot_id).toBe('b_1');

    // Switch to b_2 with a fetch we hold pending.
    const pending = deferred<CapturedPerspective>();
    fetchPerspective.mockReturnValueOnce(pending.promise);
    const inFlight = selectPerspective('b_2');

    // The selector moved immediately, but the stale b_1 data must be gone so the canvas
    // never draws b_2's ship with b_1's contacts.
    expect(get(replayPerspective)).toBe('b_2');
    expect(get(replayPerspectiveData)).toBeNull();

    pending.resolve(fakePerspective('b_2'));
    await inFlight;
    expect(get(replayPerspectiveData)?.bot_id).toBe('b_2');
  });

  it('drops a perspective fetch that resolves after a different replay is opened', async () => {
    await openReplay('A');

    // Start a fetch in replay A and hold it in flight.
    const staleFetch = deferred<CapturedPerspective>();
    fetchPerspective.mockReturnValueOnce(staleFetch.promise);
    const stale = selectPerspective('b_1');

    // Open replay B (which shares bot id b_1) while A's fetch is pending.
    await openReplay('B');
    expect(get(replayPerspective)).toBe('overall');

    // The stale fetch from replay A now resolves — it must not touch replay B's view.
    staleFetch.resolve(fakePerspective('b_1'));
    await stale;
    expect(get(replayPerspectiveData)).toBeNull();

    // Selecting b_1 in replay B must trigger a real fetch (the cache was not poisoned by A).
    const freshFetch = deferred<CapturedPerspective>();
    fetchPerspective.mockReturnValueOnce(freshFetch.promise);
    const fresh = selectPerspective('b_1');
    freshFetch.resolve(fakePerspective('b_1'));
    await fresh;

    expect(get(replayPerspectiveData)?.bot_id).toBe('b_1');
    // The last perspective fetch was for replay B — not served from a poisoned cache.
    const calls = fetchPerspective.mock.calls;
    expect(calls[calls.length - 1][0]).toBe('B');
  });
});
