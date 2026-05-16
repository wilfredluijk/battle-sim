import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  AdminWsClient,
  type AdminConnState,
  type WebSocketFactory,
} from '../lib/adminWsClient';
import type { AdminServerMsg } from '../types/protocol';

class FakeSocket {
  static OPEN = 1;
  readyState: number = 0;
  onopen: ((this: WebSocket, ev: Event) => unknown) | null = null;
  onclose: ((this: WebSocket, ev: CloseEvent) => unknown) | null = null;
  onerror: ((this: WebSocket, ev: Event) => unknown) | null = null;
  onmessage: ((this: WebSocket, ev: MessageEvent) => unknown) | null = null;
  sent: string[] = [];
  closed = false;
  constructor(public url: string) {}
  send(data: string): void {
    this.sent.push(data);
  }
  close = vi.fn(() => {
    this.closed = true;
  });
  _open(): void {
    this.readyState = FakeSocket.OPEN;
    this.onopen?.call(this as unknown as WebSocket, new Event('open'));
  }
  _message(msg: AdminServerMsg | string): void {
    const data = typeof msg === 'string' ? msg : JSON.stringify(msg);
    this.onmessage?.call(this as unknown as WebSocket, { data } as MessageEvent);
  }
  _close(): void {
    this.closed = true;
    this.readyState = 3;
    this.onclose?.call(this as unknown as WebSocket, new CloseEvent('close'));
  }
}

function makeFactory(): { factory: WebSocketFactory; sockets: FakeSocket[] } {
  const sockets: FakeSocket[] = [];
  const factory: WebSocketFactory = (url) => {
    const s = new FakeSocket(url);
    sockets.push(s);
    return s as unknown as WebSocket;
  };
  return { factory, sockets };
}

describe('AdminWsClient', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('connects, receives state, and reports authed with the payload', () => {
    const { factory, sockets } = makeFactory();
    const client = new AdminWsClient('ws://test/admin?token=t', { factory });
    const states: AdminConnState[] = [];
    client.onConn((s) => states.push(s));
    client.start();

    expect(states.at(-1)?.kind).toBe('connecting');
    sockets[0]!._open();
    sockets[0]!._message({
      type: 'state',
      room: 'main',
      state: 'lobby',
      tick: 0,
      last_winner: null,
      bots: [],
    });

    const last = states.at(-1)!;
    expect(last.kind).toBe('authed');
    if (last.kind === 'authed') {
      expect(last.state.room).toBe('main');
      expect(last.state.state).toBe('lobby');
    }
  });

  it('close before any state frame reports rejected (auth failure)', () => {
    const { factory, sockets } = makeFactory();
    const client = new AdminWsClient('ws://test/admin?token=bad', { factory });
    const states: AdminConnState[] = [];
    client.onConn((s) => states.push(s));
    client.start();

    sockets[0]!._close();
    const last = states.at(-1)!;
    expect(last.kind).toBe('rejected');
  });

  it('does not reconnect after auth failure', () => {
    const { factory, sockets } = makeFactory();
    const client = new AdminWsClient('ws://test/admin?token=bad', {
      factory,
      reconnectDelayMs: 100,
    });
    client.start();
    sockets[0]!._close();
    vi.advanceTimersByTime(10_000);
    expect(sockets.length).toBe(1);
  });

  it('reconnects after post-auth disconnect', () => {
    const { factory, sockets } = makeFactory();
    const client = new AdminWsClient('ws://test/admin?token=t', {
      factory,
      reconnectDelayMs: 500,
    });
    client.start();
    sockets[0]!._open();
    sockets[0]!._message({
      type: 'state',
      room: 'main',
      state: 'lobby',
      tick: 0,
      bots: [],
    });
    sockets[0]!._close();

    vi.advanceTimersByTime(499);
    expect(sockets.length).toBe(1);
    vi.advanceTimersByTime(1);
    expect(sockets.length).toBe(2);
  });

  it('sendStart / sendKick emit the correct typed frames', () => {
    const { factory, sockets } = makeFactory();
    const client = new AdminWsClient('ws://test/admin?token=t', { factory });
    client.start();
    sockets[0]!._open();
    sockets[0]!._message({
      type: 'state',
      room: 'main',
      state: 'lobby',
      tick: 0,
      bots: [],
    });

    client.sendStart();
    client.sendKick('b_3');
    expect(sockets[0]!.sent).toEqual([
      JSON.stringify({ type: 'start' }),
      JSON.stringify({ type: 'kick', bot_id: 'b_3' }),
    ]);
  });

  it('ignores unknown frame types without breaking the connection', () => {
    const { factory, sockets } = makeFactory();
    const client = new AdminWsClient('ws://test/admin?token=t', { factory });
    const states: AdminConnState[] = [];
    client.onConn((s) => states.push(s));
    client.start();
    sockets[0]!._open();
    sockets[0]!._message('not json');
    sockets[0]!._message({ type: 'unknown' } as unknown as AdminServerMsg);

    // Still in `connecting` because we never got a valid state frame.
    expect(states.at(-1)?.kind).toBe('connecting');

    sockets[0]!._message({
      type: 'state',
      room: 'main',
      state: 'lobby',
      tick: 0,
      bots: [],
    });
    expect(states.at(-1)?.kind).toBe('authed');
  });
});
