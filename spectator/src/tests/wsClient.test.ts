import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WsClient, type WebSocketFactory } from '../lib/wsClient';
import type { WorldFrame } from '../types/protocol';

// Minimal fake WebSocket — driven by tests calling `_open()` / `_message()` / `_close()`.
class FakeSocket {
  onopen: ((this: WebSocket, ev: Event) => unknown) | null = null;
  onclose: ((this: WebSocket, ev: CloseEvent) => unknown) | null = null;
  onerror: ((this: WebSocket, ev: Event) => unknown) | null = null;
  onmessage: ((this: WebSocket, ev: MessageEvent) => unknown) | null = null;
  closed = false;
  constructor(public url: string) {}
  close = vi.fn(() => {
    this.closed = true;
  });
  _open(): void {
    this.onopen?.call(this as unknown as WebSocket, new Event('open'));
  }
  _message(data: unknown): void {
    const event = { data: typeof data === 'string' ? data : JSON.stringify(data) } as MessageEvent;
    this.onmessage?.call(this as unknown as WebSocket, event);
  }
  _close(): void {
    this.closed = true;
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

describe('WsClient', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('emits connecting → live on open', () => {
    const { factory, sockets } = makeFactory();
    const client = new WsClient('ws://test/spectate', { factory });
    const statuses: string[] = [];
    client.onStatus((s) => statuses.push(s.message));
    client.start();
    expect(statuses).toEqual(['connecting…']);
    sockets[0]!._open();
    expect(statuses).toEqual(['connecting…', 'live']);
  });

  it('delivers parsed world frames to onWorld listeners', () => {
    const { factory, sockets } = makeFactory();
    const client = new WsClient('ws://test/spectate', { factory });
    const received: WorldFrame[] = [];
    client.onWorld((f) => received.push(f));
    client.start();
    sockets[0]!._open();

    const frame: WorldFrame = {
      type: 'world',
      tick: 7,
      ships: [],
      shells: [],
      events: [],
    };
    sockets[0]!._message(frame);
    expect(received).toEqual([frame]);
  });

  it('ignores non-world frames and malformed JSON without breaking the stream', () => {
    const { factory, sockets } = makeFactory();
    const client = new WsClient('ws://test/spectate', { factory });
    const received: WorldFrame[] = [];
    client.onWorld((f) => received.push(f));
    client.start();
    sockets[0]!._open();

    sockets[0]!._message('not json');
    sockets[0]!._message({ type: 'something_else' });
    expect(received).toEqual([]);

    const frame: WorldFrame = {
      type: 'world',
      tick: 1,
      ships: [],
      shells: [],
      events: [],
    };
    sockets[0]!._message(frame);
    expect(received).toEqual([frame]);
  });

  it('reconnects after close once the delay elapses', () => {
    const { factory, sockets } = makeFactory();
    const client = new WsClient('ws://test/spectate', { factory, reconnectDelayMs: 500 });
    client.start();
    sockets[0]!._open();
    sockets[0]!._close();
    expect(sockets.length).toBe(1);

    vi.advanceTimersByTime(499);
    expect(sockets.length).toBe(1);

    vi.advanceTimersByTime(1);
    expect(sockets.length).toBe(2);
  });

  it('close() cancels a pending reconnect and prevents future connects', () => {
    const { factory, sockets } = makeFactory();
    const client = new WsClient('ws://test/spectate', { factory, reconnectDelayMs: 500 });
    client.start();
    sockets[0]!._open();
    sockets[0]!._close();

    client.close();
    vi.advanceTimersByTime(10_000);
    expect(sockets.length).toBe(1);
  });

  it('close() while connected calls socket.close()', () => {
    const { factory, sockets } = makeFactory();
    const client = new WsClient('ws://test/spectate', { factory });
    client.start();
    sockets[0]!._open();

    client.close();
    expect(sockets[0]!.close).toHaveBeenCalledTimes(1);
  });
});
