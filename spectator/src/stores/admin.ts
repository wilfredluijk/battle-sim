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

/** Set when the room poll fails (server unreachable); cleared on the next success. */
export const roomError = writable<string | null>(null);

/** Parameter-form metadata from `GET /api/config/schema`. Fetched once at startup. */
export const configSchema = writable<ConfigField[]>([]);

/** Most recent match report, or null when no match has finished. */
export const report = writable<MatchReport | null>(null);

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
export function startControlPlane(): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let prevState: string | null = null;

  api
    .fetchConfigSchema()
    .then((fields) => configSchema.set(fields))
    .catch(() => {
      /* schema is best-effort; the form falls back to whatever the server reports */
    });

  const poll = async (): Promise<void> => {
    if (stopped) return;
    try {
      const info = await api.fetchRoom();
      if (stopped) return;
      room.set(info);
      roomError.set(null);

      // A running → not-running transition means a match just finished: surface its
      // report. A new match (→ running) clears the report screen.
      if (prevState === 'running' && info.state !== 'running') {
        showReport.set(true);
      } else if (info.state === 'running') {
        showReport.set(false);
      }
      prevState = info.state;

      if (info.state === 'running') {
        report.set(null);
      } else {
        try {
          report.set(await api.fetchReport());
        } catch {
          /* keep the previous report on a transient failure */
        }
      }
    } catch (e) {
      if (!stopped) {
        roomError.set(e instanceof Error ? e.message : 'room unavailable');
      }
    }
    if (!stopped) timer = setTimeout(poll, POLL_MS);
  };
  void poll();

  return () => {
    stopped = true;
    if (timer != null) clearTimeout(timer);
  };
}

/** Re-fetch room state immediately so the UI reflects an action without waiting a poll. */
async function refreshRoom(): Promise<void> {
  try {
    room.set(await api.fetchRoom());
    roomError.set(null);
  } catch {
    /* the next scheduled poll will retry */
  }
}

/** Run an authenticated action; clears the token on a 401 so the UI prompts for re-login. */
async function withToken<T>(fn: (token: string) => Promise<T>): Promise<T> {
  const token = get(adminToken);
  if (!token) throw new ApiError(401, 'unauthorized', 'log in as admin first');
  try {
    return await fn(token);
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      adminToken.set(null);
    }
    throw e;
  }
}

// ---------------------------------------------------------------------------
// Action helpers — each throws `ApiError` on failure for the caller to surface.
// ---------------------------------------------------------------------------

export async function loginAdmin(password: string): Promise<void> {
  const { token } = await api.login(password);
  adminToken.set(token);
}

export function logoutAdmin(): void {
  adminToken.set(null);
}

export async function applyConfig(config: SimConfig): Promise<void> {
  await withToken((t) => api.putConfig(t, config));
  await refreshRoom();
}

export async function startMatch(): Promise<void> {
  await withToken(api.startMatch);
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
  let timer: ReturnType<typeof setTimeout> | null = null;

  const poll = async (): Promise<void> => {
    if (stopped) return;
    let nextDelay = MC_POLL_IDLE_MS;
    try {
      const status = await api.fetchMonteCarloStatus();
      if (stopped) return;
      mcStatus.set(status);
      nextDelay = status.running ? MC_POLL_RUNNING_MS : MC_POLL_IDLE_MS;
    } catch {
      // Server unreachable; back off to the idle cadence and retry. The room store's
      // own error handling already surfaces "server unreachable" via roomError.
    }
    if (!stopped) timer = setTimeout(poll, nextDelay);
  };
  void poll();

  return () => {
    stopped = true;
    if (timer != null) clearTimeout(timer);
  };
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
