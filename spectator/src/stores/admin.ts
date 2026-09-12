// Control-plane store: polls the public `GET /api/room` for lifecycle state, holds the
// admin JWT, and exposes typed action helpers wrapping the REST client. The spectator
// `world` stream is handled separately in `stores/index.ts`.

import { get, writable } from 'svelte/store';
import * as api from '../lib/adminApi';
import { ApiError } from '../lib/adminApi';
import type {
  ConfigField,
  MatchReport,
  McStartRequest,
  McStatus,
  RoomInfo,
  SimConfig,
} from '../types/protocol';

const TOKEN_KEY = 'naval.adminToken';

/** How often to re-poll `GET /api/room`. Lifecycle transitions are coarse, so 1.5s is
 *  plenty — the per-tick battle view is driven by the WebSocket stream, not this poll. */
const POLL_MS = 1500;

function readStoredToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

function writeStoredToken(value: string | null): void {
  try {
    if (value == null) localStorage.removeItem(TOKEN_KEY);
    else localStorage.setItem(TOKEN_KEY, value);
  } catch {
    // private-mode browsers, etc. — silently ignore
  }
}

/** Admin JWT, or null when logged out. Mirrored to localStorage so a refresh keeps it. */
export const adminToken = writable<string | null>(readStoredToken());
adminToken.subscribe((t) => writeStoredToken(t));

/** Latest `GET /api/room` snapshot, or null before the first successful poll. */
export const room = writable<RoomInfo | null>(null);
export const training = writable<import('../types/protocol').TrainingData | null>(null);
export const trainingError = writable<string | null>(null);

/** Set when the room poll fails (server unreachable); cleared on the next success. */
export const roomError = writable<string | null>(null);

/** Parameter-form metadata from `GET /api/config/schema`. Fetched once at startup. */
export const configSchema = writable<ConfigField[]>([]);

/** Most recent match report, or null when no match has finished. */
export const report = writable<MatchReport | null>(null);
export const selectedReport = writable<MatchReport | null>(null);

/** Whether the post-battle report screen should be shown. Set true when a match ends,
 *  cleared when a new match starts or the user dismisses the report. */
export const showReport = writable<boolean>(false);

/** Most recent Monte Carlo status snapshot, or null before the first poll. */
export const mcStatus = writable<McStatus | null>(null);

/** Set when starting / stopping an MC run fails — surfaced to the user inline. */
export const mcError = writable<string | null>(null);

/**
 * Begin polling the control plane. Fetches the config schema once, then polls room state
 * on an interval. Returns a teardown function that stops the loop.
 */
export const sessionExpired = writable(false);
export const roomUpdatedAt = writable(0);
export const configDraft = writable<SimConfig | null>(null);
export const actionNotice = writable<string | null>(null);

export function expireSession(): void {
  sessionExpired.set(true);
  adminToken.set(null);
}

export function startControlPlane(): () => void {
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const unsubscribe = adminToken.subscribe(token => {
    const epoch = ++generation;
    clearTimeout(timer);
    room.set(null);
    training.set(null);
    trainingError.set(null);
    roomUpdatedAt.set(0);
    roomError.set(null);
    configSchema.set([]);
    configDraft.set(null);
    report.set(null);
    selectedReport.set(null);
    showReport.set(false);
    actionNotice.set(null);
    mcStatus.set(null);
    let previous: string | null = null;
    if (!token) return;
    const current = () => epoch === generation && get(adminToken) === token;
    const poll = async () => {
      try {
        const info = await api.fetchRoom();
        if (!current()) return;
        room.set(info);
        roomUpdatedAt.set(Date.now());
        roomError.set(null);
        if (previous === 'running' && info.state !== 'running') showReport.set(true);
        if (info.state === 'running') showReport.set(false);
        previous = info.state;
        if (info.training_revision !== undefined && get(training)?.revision !== info.training_revision) {
          try {
            const history = await api.fetchTraining();
            if (!current()) return;
            if (!get(training) || history.revision >= get(training)!.revision) training.set(history);
            trainingError.set(null);
          } catch (e) {
            if (!current()) return;
            if (e instanceof ApiError && e.status === 401) { expireSession(); return; }
            trainingError.set(e instanceof Error ? e.message : 'Session history unavailable');
          }
        }
        if (!get(configSchema).length) {
          const fields = await api.fetchConfigSchema();
          if (!current()) return;
          configSchema.set(fields);
        }
        if (info.state !== 'running') {
          const latest = await api.fetchReport();
          if (!current()) return;
          report.set(latest);
        }
      } catch (e) {
        if (!current()) return;
        if (e instanceof ApiError && e.status === 401) { expireSession(); return; }
        roomError.set(e instanceof Error ? e.message : 'Server unavailable');
      }
      if (current()) timer = setTimeout(poll, POLL_MS);
    };
    void poll();
  });
  return () => { ++generation; clearTimeout(timer); unsubscribe(); };
}

async function refreshRoom(): Promise<void> {
  const token = get(adminToken);
  try {
    const info = await api.fetchRoom();
    if (token !== get(adminToken)) return;
    room.set(info);
    roomUpdatedAt.set(Date.now());
    roomError.set(null);
  } catch (e) {
    if (token !== get(adminToken)) return;
    if (e instanceof ApiError && e.status === 401) expireSession();
    else roomError.set('Action completed; refreshing server state…');
  }
}

/** Run an authenticated action; clears the token on a 401 so the UI prompts for re-login. */
async function withToken<T>(fn: (token: string) => Promise<T>): Promise<T> {
  const token = get(adminToken);
  if (!token) throw new ApiError(401, 'unauthorized', 'log in as admin first');
  try {
    const result = await fn(token);
    if (token !== get(adminToken)) throw new Error('Session changed while the action was pending.');
    return result;
  } catch (e) {
    if (token === get(adminToken) && e instanceof ApiError && e.status === 401) {
      expireSession();
    }
    throw e;
  }
}

// ---------------------------------------------------------------------------
// Action helpers — each throws `ApiError` on failure for the caller to surface.
// ---------------------------------------------------------------------------

export async function loginAdmin(password: string): Promise<void> {
  const { token } = await api.login(password);
  sessionExpired.set(false);
  adminToken.set(token);
}

export function logoutAdmin(): void {
  sessionExpired.set(false);
  adminToken.set(null);
}

export async function applyConfig(config: SimConfig): Promise<void> {
  await withToken((t) => api.putConfig(t, config));
  await refreshRoom();
}

export async function startMatch(): Promise<void> {
  if (get(configDraft)) throw new Error('Apply or discard the draft rules before starting.');
  await withToken(api.startMatch);
  selectedReport.set(null);
  await refreshRoom();
}

export async function abortMatch(): Promise<void> {
  await withToken(api.abortMatch);
  await refreshRoom();
}

export async function resetMatch(): Promise<void> {
  await withToken(api.resetMatch);
  await refreshRoom();
}

export async function kickBot(botId: string): Promise<void> {
  await withToken((t) => api.kickBot(t, botId));
  await refreshRoom();
}

// ---------------------------------------------------------------------------
// Monte Carlo helpers.
//
// Status is polled on a separate, faster interval than the room poll so the progress
// bar feels responsive during a run. The poller auto-throttles itself when no run is
// active to avoid hammering the server in idle windows.
// ---------------------------------------------------------------------------

/** How often to re-poll `GET /api/montecarlo/status` while a run is active. */
const MC_POLL_RUNNING_MS = 500;
/** Slower poll cadence when no run is in flight — just to keep the UI in sync after a
 *  stopped/completed run is finalized. */
const MC_POLL_IDLE_MS = 2500;

/**
 * Begin polling the Monte Carlo status endpoint. Mirrors `startControlPlane`'s pattern:
 * returns a teardown that stops the loop on app unmount or HMR dispose.
 */
export function startMonteCarloPolling(): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout>;
  const poll = async () => {
    let delay = MC_POLL_IDLE_MS;
    const token = get(adminToken);
    if (token && get(room)?.capabilities?.monte_carlo) {
      try {
        const status = await api.fetchMonteCarloStatus();
        if (!stopped && token === get(adminToken)) {
          mcStatus.set(status);
          delay = status.running ? MC_POLL_RUNNING_MS : MC_POLL_IDLE_MS;
        }
      } catch (e) {
        if (token === get(adminToken) && e instanceof ApiError && e.status === 401) expireSession();
      }
    }
    if (!stopped) timer = setTimeout(poll, delay);
  };
  void poll();
  return () => { stopped = true; clearTimeout(timer); };
}

export async function startMonteCarlo(config: McStartRequest): Promise<void> {
  mcError.set(null);
  try {
    await withToken((t) => api.startMonteCarlo(t, config));
    // Refresh status immediately so the UI flips to "running" without waiting a poll.
    try {
      mcStatus.set(await api.fetchMonteCarloStatus());
    } catch {
      /* next poll will retry */
    }
  } catch (e) {
    const msg = e instanceof Error ? e.message : 'failed to start monte carlo run';
    mcError.set(msg);
    throw e;
  }
}

export async function stopMonteCarlo(forceAbort = false): Promise<void> {
  mcError.set(null);
  try {
    await withToken((t) => api.stopMonteCarlo(t, forceAbort));
    try {
      mcStatus.set(await api.fetchMonteCarloStatus());
    } catch {
      /* next poll will retry */
    }
  } catch (e) {
    const msg = e instanceof Error ? e.message : 'failed to stop monte carlo run';
    mcError.set(msg);
    throw e;
  }
}

export async function reloadTraining(): Promise<void> {
  const history = await withToken(() => api.fetchTraining());
  if (!get(training) || history.revision >= get(training)!.revision) training.set(history);
  trainingError.set(null);
}
export async function saveTraining(action: import('../types/protocol').TrainingAction, revision = get(training)?.revision): Promise<void> {
  if (revision === undefined) throw new Error('Load session history before saving.');
  const history = await withToken(t => api.updateTraining(t, revision, action));
  if (!get(training) || history.revision >= get(training)!.revision) training.set(history);
  trainingError.set(null);
  await refreshRoom();
}
