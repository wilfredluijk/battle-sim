import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import * as api from '../lib/adminApi';
import { adminToken, configDraft, configSchema, room, sessionExpired, startControlPlane, startMonteCarloPolling, startMatch, training, saveTraining } from '../stores/admin';
import { colorFor } from '../lib/palette';
import { clockText, eventText } from '../lib/presentation';
import type { RoomInfo } from '../types/protocol';
vi.mock('../lib/adminApi', async importOriginal => ({ ...await importOriginal<typeof api>(), fetchRoom: vi.fn(), fetchConfigSchema: vi.fn(), fetchReport: vi.fn(), fetchMonteCarloStatus: vi.fn(), startMatch: vi.fn(), fetchTraining: vi.fn(), updateTraining: vi.fn() }));
let teardown: (() => void)[] = [];
const info = { room: 'main', state: 'lobby', tick: 0, bots: [], config: {}, capabilities: { monte_carlo: false, manage_match: true, tournament: true } } as RoomInfo;
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks(); adminToken.set(null); configDraft.set(null); sessionExpired.set(false);
  vi.mocked(api.fetchRoom).mockResolvedValue(info);
  vi.mocked(api.fetchConfigSchema).mockResolvedValue([]);
  vi.mocked(api.fetchReport).mockResolvedValue(null);
});
afterEach(() => { teardown.forEach(stop => stop()); teardown = []; adminToken.set(null); vi.useRealTimers(); });
it('makes no protected requests while signed out or Monte Carlo unavailable', async () => {
  teardown.push(startControlPlane(), startMonteCarloPolling());
  await vi.advanceTimersByTimeAsync(6000);
  expect(api.fetchRoom).not.toHaveBeenCalled(); expect(api.fetchConfigSchema).not.toHaveBeenCalled(); expect(api.fetchMonteCarloStatus).not.toHaveBeenCalled();
  adminToken.set('test'); await vi.advanceTimersByTimeAsync(3000);
  expect(api.fetchRoom).toHaveBeenCalled(); expect(api.fetchMonteCarloStatus).not.toHaveBeenCalled();
});
it('expires the session on a protected 401 and stops polling', async () => {
  vi.mocked(api.fetchRoom).mockRejectedValue(new api.ApiError(401, 'unauthorized', 'Expired'));
  teardown.push(startControlPlane()); adminToken.set('test');
  await vi.advanceTimersByTimeAsync(5000);
  expect(get(adminToken)).toBeNull(); expect(get(sessionExpired)).toBe(true);
  expect(api.fetchRoom).toHaveBeenCalledTimes(1); expect(get(room)).toBeNull();
});
it('ignores a response that arrives after logout', async () => {
  let resolve!: (info: RoomInfo) => void;
  vi.mocked(api.fetchRoom).mockReturnValue(new Promise(yes => resolve = yes));
  teardown.push(startControlPlane()); adminToken.set('test'); adminToken.set(null);
  resolve(info); await vi.advanceTimersByTimeAsync(0);
  expect(get(room)).toBeNull(); expect(get(configSchema)).toEqual([]);
});
it('rejects starting with unapplied rules without making a mutation request', async () => {
  adminToken.set('test'); configDraft.set({ hull_hp: 150 });
  await expect(startMatch()).rejects.toThrow('Apply or discard'); expect(api.startMatch).not.toHaveBeenCalled();
});
it('assigns eight distinct team colours even for colliding name hashes', () => {
  const cache = new Map<string, string>();
  const names = ['Team Atlas', 'Harbour', 'Echo', 'Bravo', 'Charlie', 'Delta', 'Foxtrot', 'Golf'];
  const colours = names.map(name => colorFor(name, cache));
  expect(new Set(colours).size).toBe(8); expect(colorFor('Team Atlas', cache)).toBe(colours[0]);
});
it('uses team names in events and handles a non-default clock rate', () => {
  expect(eventText({ type: 'death', ship_id: 's_3' }, new Map([['s_3', 'Atlas']]))).toBe('Atlas destroyed');
  expect(clockText(3000 / 20)).toBe('2:30'); expect(clockText(-1)).toBe('0:00');
});

it('does not restore protected session history after logout', async () => {
  let resolve!: (history: import('../types/protocol').TrainingData) => void;
  vi.mocked(api.fetchRoom).mockResolvedValue({ ...info, training_revision: 1 });
  vi.mocked(api.fetchTraining).mockReturnValue(new Promise(yes => resolve = yes));
  teardown.push(startControlPlane()); adminToken.set('test');
  await vi.advanceTimersByTimeAsync(0); adminToken.set(null);
  resolve({ version: 1, revision: 1, active_session: null, sessions: [], debriefs: {}, storage_error: null });
  await vi.advanceTimersByTimeAsync(0); expect(get(training)).toBeNull();
});
it('keeps session history on a failed save and sends the draft revision for conflict detection', async () => {
  const history = { version: 1, revision: 4, active_session: null, sessions: [], debriefs: {}, storage_error: null };
  adminToken.set('test'); training.set(history);
  vi.mocked(api.updateTraining).mockRejectedValue(new api.ApiError(409, 'training_refused', 'History changed'));
  await expect(saveTraining({ action: 'activate', session_id: null }, 3)).rejects.toThrow('History changed');
  expect(api.updateTraining).toHaveBeenCalledWith('test', 3, { action: 'activate', session_id: null });
  expect(get(training)).toEqual(history);
});
