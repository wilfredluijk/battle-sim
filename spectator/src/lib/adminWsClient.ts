// Admin WebSocket client. Separate from `wsClient.ts` because the message shape
// (bidirectional typed AdminMsg / AdminServerMsg) and reconnect policy
// (no retry on auth failure, retry on post-auth disconnect) are different enough
// that sharing code would be more friction than help.

import type {
  AdminMsg,
  AdminServerMsg,
  AdminStatePayload,
} from '../types/protocol';

export type AdminConnState =
  | { kind: 'disconnected' }
  | { kind: 'connecting' }
  | { kind: 'rejected'; reason: string }
  | { kind: 'authed'; state: AdminStatePayload };

export type WebSocketFactory = (url: string) => WebSocket;

export interface AdminWsClientOptions {
  factory?: WebSocketFactory;
  /** Delay before re-attempting a post-authed reconnect. Ignored when auth failed. */
  reconnectDelayMs?: number;
}

/** Build the default admin WS URL from `window.location.host`, with the given token. */
export function defaultAdminUrl(token: string): string {
  return `ws://${window.location.host}/admin?token=${encodeURIComponent(token)}`;
}

/**
 * Auto-reconnecting admin client. On open: waits for the first `state` frame.
 *
 * - If the socket closes before any `state` frame arrives, we treat that as auth
 *   failure (the server's 401 closes the upgrade immediately). The client stops
 *   reconnecting and emits `{kind: 'rejected'}`.
 * - If the socket closes after at least one `state` frame, the server probably
 *   restarted; reconnect after `reconnectDelayMs`. The token is unchanged — if it
 *   has rotated, the next connect will fail with auth and we fall back to rejected.
 *
 * `state` events update `AdminConnState.state` in-place so consumers can render a
 * live snapshot from a single store.
 */
export class AdminWsClient {
  private readonly url: string;
  private readonly factory: WebSocketFactory;
  private readonly reconnectDelayMs: number;
  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private closed = false;
  private receivedAnyState = false;
  private currentState: AdminStatePayload | null = null;
  private connListeners = new Set<(s: AdminConnState) => void>();

  constructor(url: string, options: AdminWsClientOptions = {}) {
    this.url = url;
    this.factory = options.factory ?? ((u) => new WebSocket(u));
    this.reconnectDelayMs = options.reconnectDelayMs ?? 2000;
  }

  start(): void {
    this.closed = false;
    this.connect();
  }

  close(): void {
    this.closed = true;
    if (this.reconnectTimer != null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.socket) {
      try {
        this.socket.close();
      } catch {
        // ignore — already closing
      }
      this.socket = null;
    }
    this.currentState = null;
    this.emit({ kind: 'disconnected' });
  }

  sendStart(): void {
    this.sendMsg({ type: 'start' });
  }

  sendAbort(): void {
    this.sendMsg({ type: 'abort' });
  }

  sendReset(): void {
    this.sendMsg({ type: 'reset' });
  }

  sendKick(botId: string): void {
    this.sendMsg({ type: 'kick', bot_id: botId });
  }

  onConn(cb: (s: AdminConnState) => void): () => void {
    this.connListeners.add(cb);
    return () => this.connListeners.delete(cb);
  }

  private sendMsg(msg: AdminMsg): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return;
    try {
      this.socket.send(JSON.stringify(msg));
    } catch (e) {
      console.warn('admin send failed', e);
    }
  }

  private connect(): void {
    if (this.closed) return;
    this.receivedAnyState = false;
    this.emit({ kind: 'connecting' });

    let ws: WebSocket;
    try {
      ws = this.factory(this.url);
    } catch (e) {
      console.warn('admin ws factory threw', e);
      this.emit({ kind: 'rejected', reason: 'ws unavailable' });
      return;
    }
    this.socket = ws;

    ws.onopen = () => {
      // We don't move to 'authed' yet — wait for the first `state` frame so the UI
      // knows the auth succeeded. The server pushes the snapshot immediately after
      // accepting the subscription, so this typically lands within a tick.
    };

    ws.onerror = () => {
      // onclose fires next and decides whether to retry.
    };

    ws.onclose = () => {
      this.socket = null;
      if (!this.receivedAnyState) {
        // Closed before authentication completed — most likely a 401 from the server.
        this.emit({ kind: 'rejected', reason: 'auth failed' });
        return;
      }
      // Post-auth disconnect — schedule a reconnect.
      this.currentState = null;
      this.emit({ kind: 'disconnected' });
      this.scheduleReconnect();
    };

    ws.onmessage = (ev: MessageEvent) => {
      let parsed: unknown;
      try {
        parsed = JSON.parse(typeof ev.data === 'string' ? ev.data : String(ev.data));
      } catch (e) {
        console.warn('admin: malformed JSON', e);
        return;
      }
      if (!isAdminServerMsg(parsed)) return;
      const msg = parsed as AdminServerMsg;

      switch (msg.type) {
        case 'state': {
          this.receivedAnyState = true;
          this.currentState = {
            room: msg.room,
            state: msg.state,
            tick: msg.tick,
            last_winner: msg.last_winner ?? null,
            bots: msg.bots,
          };
          this.emit({ kind: 'authed', state: this.currentState });
          break;
        }
        case 'ack':
          // Could surface in UI; for now we rely on the next `state` push.
          break;
        case 'error':
          console.warn('admin server error', msg.code, msg.message);
          break;
      }
    };
  }

  private scheduleReconnect(): void {
    if (this.closed) return;
    if (this.reconnectTimer != null) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, this.reconnectDelayMs);
  }

  private emit(s: AdminConnState): void {
    for (const cb of this.connListeners) cb(s);
  }
}

function isAdminServerMsg(value: unknown): value is AdminServerMsg {
  if (typeof value !== 'object' || value === null) return false;
  const t = (value as { type?: unknown }).type;
  return t === 'state' || t === 'ack' || t === 'error';
}
