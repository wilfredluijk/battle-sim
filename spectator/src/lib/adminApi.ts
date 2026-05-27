// REST client for the `/api/*` control plane. Replaces the old admin WebSocket: room
// lifecycle and parameter changes are plain request/response now, so a thin `fetch`
// wrapper is all that's needed. The streaming surfaces (`/spectate`, `/bot`) stay on
// WebSocket — see `wsClient.ts`.

import type {
  ConfigField,
  MatchReport,
  McStartRequest,
  McStatus,
  RoomInfo,
  SimConfig,
} from '../types/protocol';

/** A failed REST call. Carries the HTTP status and the server's `{ code, message }`. */
export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/** Turn a non-2xx `Response` into an `ApiError`, reading the JSON error body if present. */
async function toError(res: Response): Promise<ApiError> {
  let code = 'error';
  let message = `request failed (HTTP ${res.status})`;
  try {
    const body: unknown = await res.json();
    if (body && typeof body === 'object') {
      const b = body as { code?: unknown; message?: unknown };
      if (typeof b.code === 'string') code = b.code;
      if (typeof b.message === 'string') message = b.message;
    }
  } catch {
    // Non-JSON body — keep the generic message.
  }
  return new ApiError(res.status, code, message);
}

function authHeaders(token: string): Record<string, string> {
  return { Authorization: `Bearer ${token}` };
}

// ---------------------------------------------------------------------------
// Public reads — no token required.
// ---------------------------------------------------------------------------

/** `GET /api/room` — current room lifecycle state plus the active balance parameters. */
export async function fetchRoom(): Promise<RoomInfo> {
  const res = await fetch('/api/room');
  if (!res.ok) throw await toError(res);
  return (await res.json()) as RoomInfo;
}

/** `GET /api/room/report` — the most recent match report, or `null` if none exists yet. */
export async function fetchReport(): Promise<MatchReport | null> {
  const res = await fetch('/api/room/report');
  if (res.status === 404) return null;
  if (!res.ok) throw await toError(res);
  return (await res.json()) as MatchReport;
}

/** `GET /api/config/schema` — metadata for the pre-match parameter form. */
export async function fetchConfigSchema(): Promise<ConfigField[]> {
  const res = await fetch('/api/config/schema');
  if (!res.ok) throw await toError(res);
  const body = (await res.json()) as { fields?: ConfigField[] };
  return body.fields ?? [];
}

// ---------------------------------------------------------------------------
// Auth.
// ---------------------------------------------------------------------------

export interface LoginResult {
  token: string;
  expires_at: number;
}

/** `POST /api/login` — exchange the admin password for a JWT. */
export async function login(password: string): Promise<LoginResult> {
  const res = await fetch('/api/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ password }),
  });
  if (!res.ok) throw await toError(res);
  return (await res.json()) as LoginResult;
}

// ---------------------------------------------------------------------------
// Admin mutations — require a bearer token.
// ---------------------------------------------------------------------------

/** `PUT /api/room/config` — replace the match parameters. Only valid in the lobby. */
export async function putConfig(token: string, config: SimConfig): Promise<void> {
  const res = await fetch('/api/room/config', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json', ...authHeaders(token) },
    body: JSON.stringify(config),
  });
  if (!res.ok) throw await toError(res);
}

async function postAction(token: string, path: string): Promise<void> {
  const res = await fetch(path, { method: 'POST', headers: authHeaders(token) });
  if (!res.ok) throw await toError(res);
}

/** `POST /api/room/start` — transition lobby → running. */
export function startMatch(token: string): Promise<void> {
  return postAction(token, '/api/room/start');
}

/** `POST /api/room/abort` — force-end a running match. */
export function abortMatch(token: string): Promise<void> {
  return postAction(token, '/api/room/abort');
}

/** `POST /api/room/reset` — cut the post-game pause short and return to the lobby. */
export function resetMatch(token: string): Promise<void> {
  return postAction(token, '/api/room/reset');
}

/** `POST /api/room/kick` — disconnect a bot by id. */
export async function kickBot(token: string, botId: string): Promise<void> {
  const res = await fetch('/api/room/kick', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders(token) },
    body: JSON.stringify({ bot_id: botId }),
  });
  if (!res.ok) throw await toError(res);
}

// ---------------------------------------------------------------------------
// Monte Carlo batch runner.
// ---------------------------------------------------------------------------

export interface McStartResult {
  run_id: string;
}

/** `POST /api/montecarlo/start` — kick off a batch of matches with varied positions. */
export async function startMonteCarlo(
  token: string,
  config: McStartRequest,
): Promise<McStartResult> {
  const res = await fetch('/api/montecarlo/start', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders(token) },
    body: JSON.stringify(config),
  });
  if (!res.ok) throw await toError(res);
  return (await res.json()) as McStartResult;
}

/** `POST /api/montecarlo/stop` — halt the active Monte Carlo run. */
export async function stopMonteCarlo(
  token: string,
  forceAbort = false,
): Promise<void> {
  const res = await fetch('/api/montecarlo/stop', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders(token) },
    body: JSON.stringify({ force_abort: forceAbort }),
  });
  if (!res.ok) throw await toError(res);
}

/** `GET /api/montecarlo/status` — progress + results of the active or most-recent run. */
export async function fetchMonteCarloStatus(): Promise<McStatus> {
  const res = await fetch('/api/montecarlo/status');
  if (!res.ok) throw await toError(res);
  return (await res.json()) as McStatus;
}
