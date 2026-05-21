import { describe, expect, it } from 'vitest';
import { extractMarkers } from '../lib/replayMarkers';
import type { ShellSnapshot, TickEvent, WorldFrame } from '../types/protocol';

function shell(id: number): ShellSnapshot {
  return { id_index: id, pos: [0, 0], vel: [0, 0], ttl_ticks: 30 };
}

function frame(
  tick: number,
  shells: ShellSnapshot[],
  events: TickEvent[] = [],
): WorldFrame {
  return { type: 'world', tick, ships: [], shells, events };
}

describe('extractMarkers', () => {
  it('returns no markers for an idle timeline', () => {
    const frames = [frame(0, []), frame(1, []), frame(2, [])];
    expect(extractMarkers(frames)).toEqual([]);
  });

  it('flags a fired marker when a new shell id appears', () => {
    const frames = [
      frame(0, []),
      frame(1, [shell(7)]), // shell 7 is new -> a shot was fired
      frame(2, [shell(7)]), // same shell still in flight -> not a new fire
    ];
    expect(extractMarkers(frames)).toEqual([{ tick: 1, kind: 'fired' }]);
  });

  it('treats each distinct new shell id as its own fire event', () => {
    const frames = [
      frame(0, [shell(1)]), // new at tick 0
      frame(1, [shell(1), shell(2)]), // shell 2 is new
    ];
    expect(extractMarkers(frames)).toEqual([
      { tick: 0, kind: 'fired' },
      { tick: 1, kind: 'fired' },
    ]);
  });

  it('extracts hit and kill markers from frame events', () => {
    const frames = [
      frame(0, []),
      frame(5, [], [{ type: 'hit', ship_id: 's_2', amount: 12 }]),
      frame(9, [], [{ type: 'death', ship_id: 's_2' }]),
    ];
    expect(extractMarkers(frames)).toEqual([
      { tick: 5, kind: 'hit' },
      { tick: 9, kind: 'kill' },
    ]);
  });

  it('emits at most one marker per kind per tick', () => {
    const frames = [
      frame(3, [shell(1)], [
        { type: 'hit', ship_id: 's_1', amount: 5 },
        { type: 'hit', ship_id: 's_2', amount: 8 },
      ]),
    ];
    expect(extractMarkers(frames)).toEqual([
      { tick: 3, kind: 'fired' },
      { tick: 3, kind: 'hit' },
    ]);
  });
});
