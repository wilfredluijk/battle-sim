import { describe, expect, it } from 'vitest';
import { botsInOrder, reconcile } from '../lib/worldFrame';
import { MAX_EVENTS } from '../lib/constants';
import type { ShipSnapshot, WorldFrame } from '../types/protocol';

function ship(id: string, overrides: Partial<ShipSnapshot> = {}): ShipSnapshot {
  return {
    id,
    bot_name: id,
    pos: [0, 0],
    heading_deg: 0,
    speed: 0,
    hp: 100,
    ammo: 20,
    throttle: 0,
    rudder: 0,
    alive: true,
    ready: true,
    commands_per_sec: 0,
    sensor_mode: 'passive',
    ...overrides,
  };
}

function frame(tick: number, ships: ShipSnapshot[], events: WorldFrame['events'] = []): WorldFrame {
  return { type: 'world', tick, ships, shells: [], events };
}

describe('reconcile', () => {
  it('assigns firstSeenOrder in encounter order and keeps it stable across frames', () => {
    let state = reconcile(frame(1, [ship('s1'), ship('s2')]), new Map(), [], [], 0);
    expect(state.bots.get('s1')?.firstSeenOrder).toBe(0);
    expect(state.bots.get('s2')?.firstSeenOrder).toBe(1);

    // s3 joins later — gets the next slot. s1's order is unchanged.
    state = reconcile(frame(2, [ship('s1'), ship('s2'), ship('s3')]), state.bots, state.events, state.splashes, 0);
    expect(state.bots.get('s1')?.firstSeenOrder).toBe(0);
    expect(state.bots.get('s3')?.firstSeenOrder).toBe(2);
  });

  it('marks bots not in the current frame as disconnected and keeps them in the map', () => {
    let state = reconcile(frame(1, [ship('s1'), ship('s2')]), new Map(), [], [], 0);
    state = reconcile(frame(2, [ship('s1')]), state.bots, state.events, state.splashes, 0);

    expect(state.bots.get('s1')?.connected).toBe(true);
    expect(state.bots.get('s2')?.connected).toBe(false);
    // s2's last-known snapshot is still available so the sidebar can show its final state.
    expect(state.bots.get('s2')?.ship.id).toBe('s2');
  });

  it('preserves stable sidebar order via botsInOrder', () => {
    let state = reconcile(frame(1, [ship('s1'), ship('s2'), ship('s3')]), new Map(), [], [], 0);
    state = reconcile(frame(2, [ship('s3'), ship('s1'), ship('s2')]), state.bots, state.events, state.splashes, 0);
    const ordered = botsInOrder(state.bots);
    expect(ordered.map((b) => b.ship.id)).toEqual(['s1', 's2', 's3']);
  });

  it('appends splashes for shell_splash events using the supplied timestamp', () => {
    const state = reconcile(
      frame(5, [], [{ type: 'shell_splash', pos: [42, 99] }]),
      new Map(),
      [],
      [],
      1234,
    );
    expect(state.splashes).toEqual([{ x: 42, y: 99, startedAt: 1234 }]);
  });

  it('prepends new event lines and caps the log at MAX_EVENTS', () => {
    const seed: string[] = [];
    for (let i = 0; i < MAX_EVENTS; i++) seed.push(`old line ${i}`);

    const state = reconcile(
      frame(7, [], [
        { type: 'hit', ship_id: 's1', amount: 5 },
        { type: 'death', ship_id: 's1' },
      ]),
      new Map(),
      seed,
      [],
      0,
    );
    expect(state.events.length).toBe(MAX_EVENTS);
    expect(state.events[0]).toBe('[t7] s1 destroyed'); // most recent event first
    expect(state.events[1]).toBe('[t7] hit s1 (-5)');
  });
});
