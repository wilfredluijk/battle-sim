import { writable } from 'svelte/store';
import { WsClient, defaultSpectatorUrl, type ConnectionStatus } from '../lib/wsClient';
import { reconcile, type BotCardState } from '../lib/worldFrame';
import type { Splash } from '../lib/renderer';
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

/** Top-level screen the app shows. `live` is the spectator/lobby/report flow; the replay
 * modes are an independent surface that does not touch the live `/spectate` connection;
 * `monte-carlo` opens the batch-runner panel. */
export type AppMode =
  | 'live'
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

  client.start();

  return () => {
    offStatus();
    offWorld();
    client.close();
  };
}
