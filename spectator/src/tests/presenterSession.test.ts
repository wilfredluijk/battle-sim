import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import type { WorldFrame, RoomInfo } from '../types/protocol';
import type { AudioStatus } from '../lib/presenterAudio';
const mock = vi.hoisted(() => ({
  world: null as ((frame: WorldFrame) => void) | null,
  status: null as ((status: { connected: boolean; message: string }) => void) | null,
  audioStatus: null as ((status: AudioStatus) => void) | null,
  play: vi.fn(), stop: vi.fn(), dispose: vi.fn(),
}));
vi.mock('../lib/wsClient', () => ({
  defaultSpectatorUrl: () => 'ws://example.test/spectate',
  WsClient: class {
    onStatus(fn: typeof mock.status) { mock.status = fn; return () => {}; }
    onWorld(fn: typeof mock.world) { mock.world = fn; return () => {}; }
    start() {} close() {}
  },
}));
vi.mock('../lib/presenterAudio', async importOriginal => ({
  ...await importOriginal<typeof import('../lib/presenterAudio')>(),
  PresenterAudio: class {
    constructor(changed: typeof mock.audioStatus) { mock.audioStatus = changed; }
    enable() { mock.audioStatus?.({ enabled: true, pending: false, error: null }); return Promise.resolve(); }
    mute() { mock.audioStatus?.({ enabled: false, pending: false, error: null }); }
    play = mock.play; stop = mock.stop; dispose = mock.dispose; setVolume() {}
  },
}));
import { appMode, startSpectator } from '../stores';
import { adminToken, room, roomError, roomUpdatedAt } from '../stores/admin';
import { countdownSound, enableSounds, muteSounds, soundStatus, updateSoundSettings, watchSoundVisibility } from '../stores/presenter';
let stop: () => void;
const info = { match_id: 'live-round', state: 'running', bots: [], capabilities: { manage_match: true } } as unknown as RoomInfo;
function emit(tick: number, death = false) {
  mock.world?.({ type: 'world', tick, ships: [], shells: [], events: death ? [{ type: 'death', ship_id: `s_${tick}` }] : [] });
}
beforeEach(async () => {
  vi.clearAllMocks();
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  adminToken.set('local-test'); appMode.set('live'); room.set(info); roomError.set(null); roomUpdatedAt.set(Date.now());
  updateSoundSettings({ countdown: true, eliminations: true, volume: 0.25 });
  stop = startSpectator(); await enableSounds();
});
afterEach(() => { stop(); adminToken.set(null); localStorage.clear(); });
it('plays fresh live deaths and keeps replay navigation, server replay and reconnect quiet', () => {
  emit(1); emit(2, true); expect(mock.play).toHaveBeenCalledOnce(); mock.play.mockClear();
  appMode.set('replay-viewer'); emit(3, true); emit(4, true); expect(mock.play).not.toHaveBeenCalled();
  appMode.set('live'); emit(5, true); expect(mock.play).not.toHaveBeenCalled();
  emit(6, true); expect(mock.play).toHaveBeenCalledOnce(); mock.play.mockClear();
  mock.status?.({ connected: false, message: 'retrying' }); emit(7, true); expect(mock.play).not.toHaveBeenCalled();
  room.set({ ...info, replay_mode: true }); emit(8, true); emit(9, true); expect(mock.play).not.toHaveBeenCalled();
});
it('suppresses stale room data, disabled cue categories, muted and signed-out events', () => {
  emit(1); roomUpdatedAt.set(Date.now() - 6000); emit(2, true);
  roomUpdatedAt.set(Date.now()); roomError.set('Disconnected'); emit(3, true); roomError.set(null);
  updateSoundSettings({ eliminations: false, countdown: false }); emit(4); emit(5, true); countdownSound();
  expect(mock.play).not.toHaveBeenCalled();
  updateSoundSettings({ eliminations: true }); emit(6, true); expect(mock.play).not.toHaveBeenCalled();
  emit(7, true); expect(mock.play).toHaveBeenCalledOnce(); mock.play.mockClear();
  muteSounds(); emit(8, true); expect(mock.play).not.toHaveBeenCalled();
  adminToken.set(null); expect(get(soundStatus).enabled).toBe(false);
});
it('mutes on tab hiding and requires a new gesture after returning', async () => {
  const unwatch = watchSoundVisibility();
  Object.defineProperty(document, 'hidden', { configurable: true, value: true });
  document.dispatchEvent(new Event('visibilitychange'));
  countdownSound(); expect(mock.play).not.toHaveBeenCalled(); expect(get(soundStatus).enabled).toBe(false);
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  document.dispatchEvent(new Event('visibilitychange')); expect(get(soundStatus).enabled).toBe(false);
  await enableSounds(); emit(1, true); expect(mock.play).not.toHaveBeenCalled();
  emit(2, true); expect(mock.play).toHaveBeenCalledOnce();
  unwatch(); expect(mock.dispose).toHaveBeenCalledOnce();
});
