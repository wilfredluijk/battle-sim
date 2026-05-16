import { writable } from 'svelte/store';
import {
  AdminWsClient,
  defaultAdminUrl,
  type AdminConnState,
} from '../lib/adminWsClient';
import type { AdminStatePayload } from '../types/protocol';

const TOKEN_KEY = 'naval.adminToken';

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

/** Admin token entered by the user. Mirrored to localStorage so refresh preserves it. */
export const adminToken = writable<string | null>(readStoredToken());
adminToken.subscribe((t) => writeStoredToken(t));

/** Live connection state for the admin endpoint. `disconnected` until `startAdmin` runs. */
export const adminConn = writable<AdminConnState>({ kind: 'disconnected' });

/** Most recent room state pushed by the server, or null when not authed. */
export const adminRoom = writable<AdminStatePayload | null>(null);

/** Module-scoped client so commands sent from the panel reach the same socket. */
let client: AdminWsClient | null = null;

/**
 * Start the admin WS client with the given token and wire its events into the stores.
 * Returns a teardown function that closes the socket. Calling `startAdmin` while a
 * previous client is active closes the previous one first.
 */
export function startAdmin(
  token: string,
  url: string = defaultAdminUrl(token),
): () => void {
  // Stop any previous client first — e.g. when the user re-enters a token.
  if (client) {
    client.close();
    client = null;
  }
  const c = new AdminWsClient(url);
  client = c;

  const offConn = c.onConn((s) => {
    adminConn.set(s);
    if (s.kind === 'authed') {
      adminRoom.set(s.state);
    } else if (s.kind === 'rejected') {
      adminRoom.set(null);
      // Wipe the stored token so the user is prompted to re-enter it.
      adminToken.set(null);
    } else if (s.kind === 'disconnected') {
      adminRoom.set(null);
    }
  });

  c.start();

  return () => {
    offConn();
    c.close();
    if (client === c) client = null;
  };
}

export function stopAdmin(): void {
  if (client) {
    client.close();
    client = null;
  }
  adminConn.set({ kind: 'disconnected' });
  adminRoom.set(null);
}

// Convenience command senders so components don't need to hold a client handle.
export function adminSendStart(): void {
  client?.sendStart();
}
export function adminSendAbort(): void {
  client?.sendAbort();
}
export function adminSendReset(): void {
  client?.sendReset();
}
export function adminSendKick(botId: string): void {
  client?.sendKick(botId);
}
