import type { TickEvent } from '../types/protocol';

/**
 * Format a single tick event for the spectator's scrolling event log. Returns `null` for
 * event types the spectator doesn't surface (forward-compatible no-op).
 */
export function formatEvent(tick: number, ev: TickEvent | undefined | null): string | null {
  if (!ev || typeof (ev as { type?: unknown }).type !== 'string') return null;
  const t = `[t${tick}]`;
  switch (ev.type) {
    case 'hit':
      return `${t} hit ${ev.ship_id} (-${ev.amount})`;
    case 'shell_splash':
      return `${t} splash @ (${ev.pos[0].toFixed(0)}, ${ev.pos[1].toFixed(0)})`;
    case 'death':
      return `${t} ${ev.ship_id} destroyed`;
    default:
      return null;
  }
}
