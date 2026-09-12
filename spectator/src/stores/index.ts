import { adminToken, room } from './admin';
import { get } from 'svelte/store';
import { writable } from 'svelte/store';
import { WsClient, defaultSpectatorUrl, type ConnectionStatus } from '../lib/wsClient';
import { reconcile, type BotCardState } from '../lib/worldFrame';
import type { Splash } from '../lib/renderer';
import type { TimelineEvent } from '../lib/presentation';
import type { WorldFrame } from '../types/protocol';

export type ViewMode = 'split' | 'full';

export const connection = writable<ConnectionStatus>({
  connected: false,
  message: 'connecting…',
});

export const tick = writable<number>(0);
export const bots = writable<Map<string, BotCardState>>(new Map());
export const events = writable<string[]>([]);
export const splashes = writable<Splash[]>([]);

/** Latest world frame, exposed so the canvas renderer can draw every animation frame
 * without re-running reconciliation. */
export const latestWorld = writable<WorldFrame | null>(null);

export const view = writable<ViewMode>('split');
export const selectedTeam = writable<string | null>(null);
export const projector = writable(false);
export const displayOptions = writable({ radar: true, labels: true, trails: false });
export const worldUpdatedAt = writable(0);
export const structuredEvents = writable<{ tick: number; event: TimelineEvent }[]>([]);

/** Top-level screen the app shows. `live` is the spectator/lobby/report flow; the replay
 * modes are an independent surface that does not touch the live `/spectate` connection;
 * `monte-carlo` opens the batch-runner panel. */
export type AppMode =
  | 'live'
  | 'lobby'
  | 'results'
  | 'settings'
  | 'sessions'
  | 'replay-browser'
  | 'replay-viewer'
  | 'monte-carlo';
export const appMode = writable<AppMode>('live');

/**
 * Start the spectator's WebSocket client and wire its events into the reactive stores.
 * Returns a teardown function that closes the connection — call it on app unmount or
 * HMR dispose to avoid leaking sockets.
 */
export function startSpectator(url: string = defaultSpectatorUrl()): () => void {
  // Canonical state lives in this closure; the stores are just reactive projections.
  let curBots: Map<string, BotCardState> = new Map();
  let curEvents: string[] = [];
  let curSplashes: Splash[] = [];

  const client = new WsClient(url);

  const offStatus = client.onStatus((s) => connection.set(s));

  const offWorld = client.onWorld((frame) => {
    if (frame.tick < (get(latestWorld)?.tick ?? 0)) {
      curBots = new Map(); curEvents = []; curSplashes = [];
      structuredEvents.set([]);
    }
    worldUpdatedAt.set(Date.now());
    structuredEvents.update(old => [...frame.events.map(event => ({ tick: frame.tick, event })).reverse(), ...old].slice(0, 100));
    const result = reconcile(frame, curBots, curEvents, curSplashes, performance.now());
    curBots = result.bots;
    curEvents = result.events;
    curSplashes = result.splashes;

    latestWorld.set(frame);
    tick.set(frame.tick);
    bots.set(curBots);
    events.set(curEvents);
    splashes.set(curSplashes);
  });

  let lastMatchId: string | undefined;
  const reportedDisconnects = new Set<string>();
  const offRoom = room.subscribe(info => {
    if (info?.match_id !== lastMatchId) { reportedDisconnects.clear(); lastMatchId = info?.match_id; }
    for (const bot of info?.bots ?? []) {
      if (!bot.forfeited || reportedDisconnects.has(bot.bot_id)) continue;
      reportedDisconnects.add(bot.bot_id);
      structuredEvents.update(old => [{ tick: info?.tick ?? 0, event: { type: 'disconnect' as const, ship_id: bot.ship_id, reason: bot.disconnect_reason ?? 'connection closed' } }, ...old].slice(0, 100));
    }
  });

  const offAuth = adminToken.subscribe((token) => {
    client.close();
    latestWorld.set(null);
    curBots = new Map(); curEvents = []; curSplashes = [];
    bots.set(curBots); events.set([]); splashes.set([]); structuredEvents.set([]);
    tick.set(0); worldUpdatedAt.set(0); selectedTeam.set(null);
    if (token) client.start();
    else connection.set({ connected: false, message: 'log in as admin' });
  });

  return () => {
    offAuth();
    offRoom();
    offStatus();
    offWorld();
    client.close();
  };
}
