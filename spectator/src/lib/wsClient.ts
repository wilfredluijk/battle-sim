import type { WorldFrame } from '../types/protocol';

export interface ConnectionStatus {
  connected: boolean;
  message: string;
}

export type WebSocketFactory = (url: string) => WebSocket;

export interface WsClientOptions {
  factory?: WebSocketFactory;
  /** Delay before re-attempting a connection after the socket closes. */
  reconnectDelayMs?: number;
}

/**
 * Auto-reconnecting WebSocket client for the spectator. Owns one live connection at a time
 * and dispatches to two listener channels: `onStatus` for connection state changes,
 * `onWorld` for `world` frames. Anything else is logged and ignored.
 *
 * The `factory` option exists so unit tests can inject a fake WebSocket and drive
 * `open`/`message`/`close` deterministically. The default factory uses the platform
 * `WebSocket` constructor.
 *
 * `close()` is permanent: it cancels any pending reconnect timer and prevents future
 * connects until `start()` is called again. This matters for Svelte HMR — without it, a
 * reload spawns a new client and orphans the old one.
 */
export class WsClient {
  private readonly url: string;
  private readonly factory: WebSocketFactory;
  private readonly reconnectDelayMs: number;
  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private closed = false;
  private statusListeners = new Set<(s: ConnectionStatus) => void>();
  private worldListeners = new Set<(f: WorldFrame) => void>();

  constructor(url: string, options: WsClientOptions = {}) {
    this.url = url;
    this.factory = options.factory ?? ((u) => new WebSocket(u));
    this.reconnectDelayMs = options.reconnectDelayMs ?? 1500;
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
        // ignore — already closing or closed
      }
      this.socket = null;
    }
  }

  onStatus(cb: (s: ConnectionStatus) => void): () => void {
    this.statusListeners.add(cb);
    return () => this.statusListeners.delete(cb);
  }

  onWorld(cb: (f: WorldFrame) => void): () => void {
    this.worldListeners.add(cb);
    return () => this.worldListeners.delete(cb);
  }

  private connect(): void {
    if (this.closed) return;
    this.emitStatus({ connected: false, message: 'connecting…' });

    let ws: WebSocket;
    try {
      ws = this.factory(this.url);
    } catch (e) {
      console.warn('ws factory threw', e);
      this.emitStatus({ connected: false, message: 'ws unavailable' });
      this.scheduleReconnect();
      return;
    }
    this.socket = ws;

    ws.onopen = () => this.emitStatus({ connected: true, message: 'live' });
    ws.onerror = () => {
      // onclose fires next and handles the retry; do not double-schedule.
    };
    ws.onclose = () => {
      this.socket = null;
      this.emitStatus({ connected: false, message: 'disconnected — retrying' });
      this.scheduleReconnect();
    };
    ws.onmessage = (ev: MessageEvent) => {
      let msg: unknown;
      try {
        msg = JSON.parse(typeof ev.data === 'string' ? ev.data : String(ev.data));
      } catch (e) {
        console.warn('bad world frame', e);
        return;
      }
      if (isWorldFrame(msg)) {
        for (const cb of this.worldListeners) cb(msg);
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

  private emitStatus(s: ConnectionStatus): void {
    for (const cb of this.statusListeners) cb(s);
  }
}

/** Build the default spectator WebSocket URL from `window.location.host`. */
export function defaultSpectatorUrl(): string {
  return `ws://${window.location.host}/spectate`;
}

function isWorldFrame(value: unknown): value is WorldFrame {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as { type?: unknown }).type === 'world'
  );
}
