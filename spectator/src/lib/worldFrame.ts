import { MAX_EVENTS } from './constants';
import { formatEvent } from './formatEvent';
import type { Splash } from './renderer';
import type { ShipSnapshot, TickEvent, WorldFrame } from '../types/protocol';

export interface BotCardState {
  ship: ShipSnapshot;
  lastSeenTick: number;
  firstSeenOrder: number;
  connected: boolean;
}

export interface ReconcileResult {
  bots: Map<string, BotCardState>;
  events: string[];
  splashes: Splash[];
}

/**
 * Reconcile a freshly-received `world` frame against the spectator's accumulated state.
 *
 * - Bots: any ship in the frame becomes / stays connected with its latest snapshot;
 *   any ship that was previously tracked but is missing this tick flips to
 *   `connected: false` (it'll keep showing in the sidebar greyed out). `firstSeenOrder`
 *   is stable across frames — the sidebar renders in that order so new joiners always
 *   slot in at the bottom.
 * - Events: each protocol event is run through `formatEvent`; non-null lines are
 *   prepended to the log, capped at `MAX_EVENTS`.
 * - Splashes: every `shell_splash` event spawns a new `Splash` keyed at `now` so the
 *   renderer can animate it.
 *
 * Pure: returns new collections, never mutates the inputs. Splashes from `prevSplashes`
 * are passed through unchanged; the renderer is what expires them by age.
 */
export function reconcile(
  frame: WorldFrame,
  prev: Map<string, BotCardState>,
  prevEvents: string[],
  prevSplashes: Splash[],
  now: number,
): ReconcileResult {
  const next = new Map<string, BotCardState>();

  // Carry over prior state first so we can preserve `firstSeenOrder` and flip ships that
  // disappear to disconnected.
  for (const [id, card] of prev) {
    next.set(id, { ...card, connected: false });
  }

  let nextOrder = next.size;
  for (const ship of frame.ships) {
    const existing = next.get(ship.id);
    const firstSeenOrder = existing ? existing.firstSeenOrder : nextOrder++;
    next.set(ship.id, {
      ship,
      lastSeenTick: frame.tick,
      firstSeenOrder,
      connected: true,
    });
  }

  const newLines: string[] = [];
  const newSplashes: Splash[] = [];
  for (const ev of frame.events ?? []) {
    if (isShellSplash(ev)) {
      newSplashes.push({ x: ev.pos[0], y: ev.pos[1], startedAt: now });
    }
    const line = formatEvent(frame.tick, ev);
    if (line) newLines.push(line);
  }

  // New events go on top; truncate so the list never grows unbounded.
  const events = [...newLines.reverse(), ...prevEvents].slice(0, MAX_EVENTS);
  const splashes = [...prevSplashes, ...newSplashes];

  return { bots: next, events, splashes };
}

function isShellSplash(ev: TickEvent): ev is Extract<TickEvent, { type: 'shell_splash' }> {
  return ev.type === 'shell_splash';
}

/** Stable sidebar ordering by `firstSeenOrder`. */
export function botsInOrder(bots: Map<string, BotCardState>): BotCardState[] {
  return Array.from(bots.values()).sort((a, b) => a.firstSeenOrder - b.firstSeenOrder);
}
