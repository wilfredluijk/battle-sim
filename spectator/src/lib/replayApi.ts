// REST client for the read-only `/api/replays/*` routes that back the replay viewer.
// See `docs/PROTOCOL.md §2.6`.

import { ApiError } from './adminApi';
import type {
  CapturedPerspective,
  CapturedReplay,
  ReplaySummary,
} from '../types/protocol';

/** GET `url` as JSON, turning a non-2xx response into an `ApiError`. */
async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) {
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
    throw new ApiError(res.status, code, message);
  }
  return (await res.json()) as T;
}

/** `GET /api/replays` — the replays available on disk, newest first. */
export function fetchReplays(): Promise<ReplaySummary[]> {
  return getJson<ReplaySummary[]>('/api/replays');
}

/** `GET /api/replays/{id}` — the full ground-truth timeline for one replay. */
export function fetchReplay(id: string): Promise<CapturedReplay> {
  return getJson<CapturedReplay>(`/api/replays/${encodeURIComponent(id)}`);
}

/** `GET /api/replays/{id}/perspective/{bot_id}` — one bot's sensor-filtered timeline. */
export function fetchPerspective(
  id: string,
  botId: string,
): Promise<CapturedPerspective> {
  return getJson<CapturedPerspective>(
    `/api/replays/${encodeURIComponent(id)}/perspective/${encodeURIComponent(botId)}`,
  );
}
