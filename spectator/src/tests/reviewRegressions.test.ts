import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { mergeConfig } from '../lib/config';
import { monteCarloPhase } from '../lib/mcPhase';
import { fetchReplay, fetchPerspective } from '../lib/replayApi';
import { exitReplay, openReplay, selectPerspective, replayData, replayPerspectiveData, replayPerspective, replayLoading } from '../stores/replay';
import type { CapturedReplay, CapturedPerspective, McStatus } from '../types/protocol';

vi.mock('../lib/replayApi', () => ({ fetchReplay: vi.fn(), fetchPerspective: vi.fn() }));
const replay = (id: string) => ({ header: { replay_id: id }, frames: [], end: null }) as unknown as CapturedReplay;
const perspective = (id: string) => ({ bot_id: id, frames: [] }) as CapturedPerspective;
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
beforeEach(() => { exitReplay(); vi.resetAllMocks(); });

describe('replay requests', () => {
  it('clears the old perspective while loading the next one', async () => {
    vi.mocked(fetchReplay).mockResolvedValue(replay('A'));
    await openReplay('A');
    vi.mocked(fetchPerspective).mockResolvedValueOnce(perspective('b_1'));
    await selectPerspective('b_1');
    const slow = deferred<CapturedPerspective>();
    vi.mocked(fetchPerspective).mockReturnValueOnce(slow.promise);
    const loading = selectPerspective('b_2');
    expect(get(replayPerspectiveData)).toBeNull();
    slow.resolve(perspective('b_2'));
    await loading;
    expect(get(replayPerspectiveData)?.bot_id).toBe('b_2');
  });
  it('never caches a perspective from another replay', async () => {
    vi.mocked(fetchReplay).mockImplementation(async id => replay(id));
    await openReplay('A');
    const slow = deferred<CapturedPerspective>();
    vi.mocked(fetchPerspective).mockReturnValueOnce(slow.promise);
    const loading = selectPerspective('b_1');
    await openReplay('B');
    slow.resolve(perspective('old-A'));
    await loading;
    vi.mocked(fetchPerspective).mockResolvedValueOnce(perspective('new-B'));
    await selectPerspective('b_1');
    expect(fetchPerspective).toHaveBeenLastCalledWith('B', 'b_1');
    expect(get(replayPerspectiveData)?.bot_id).toBe('new-B');
  });
  it('ignores late errors after a newer perspective succeeds', async () => {
    vi.mocked(fetchReplay).mockResolvedValue(replay('A'));
    await openReplay('A');
    const slow = deferred<CapturedPerspective>();
    vi.mocked(fetchPerspective).mockReturnValueOnce(slow.promise);
    const loading = selectPerspective('b_1');
    vi.mocked(fetchPerspective).mockResolvedValueOnce(perspective('b_2'));
    await selectPerspective('b_2');
    slow.reject(new Error('old request'));
    await loading;
    expect(get(replayPerspective)).toBe('b_2');
    expect(get(replayPerspectiveData)?.bot_id).toBe('b_2');
    expect(get(replayLoading)).toBe(false);
  });
  it('ignores replay responses after exit or a newer open', async () => {
    const slow = deferred<CapturedReplay>();
    vi.mocked(fetchReplay).mockReturnValueOnce(slow.promise).mockResolvedValueOnce(replay('B'));
    const loading = openReplay('A');
    await openReplay('B');
    slow.resolve(replay('A'));
    await loading;
    expect(get(replayData)?.header.replay_id).toBe('B');
    const later = deferred<CapturedReplay>();
    vi.mocked(fetchReplay).mockReturnValueOnce(later.promise);
    const abandoned = openReplay('C');
    exitReplay();
    later.resolve(replay('C'));
    await abandoned;
    expect(get(replayData)).toBeNull();
  });
});

it('can show setup after completed runs and running takes priority', () => {
  const status = { running: false, completed: 10 } as McStatus;
  expect(monteCarloPhase(status, false)).toBe('completed');
  expect(monteCarloPhase(status, true)).toBe('setup');
  expect(monteCarloPhase({ ...status, running: true }, true)).toBe('running');
});
it('preserves hidden powerup tuning when editing flat settings', () => {
  const current = { hull_hp: 100, powerups: { awacs_range_mult: 3 } };
  const updated = mergeConfig(current, { hull_hp: 200 });
  expect(updated).toEqual({ hull_hp: 200, powerups: { awacs_range_mult: 3 } });
  expect(updated.powerups).not.toBe(current.powerups);
});
