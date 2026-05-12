import { describe, expect, it } from 'vitest';
import { formatEvent } from '../lib/formatEvent';

describe('formatEvent', () => {
  it('formats a hit event', () => {
    expect(formatEvent(42, { type: 'hit', ship_id: 's_3', amount: 12 })).toBe(
      '[t42] hit s_3 (-12)',
    );
  });

  it('formats a shell_splash with rounded coords', () => {
    expect(formatEvent(100, { type: 'shell_splash', pos: [220.6, 505.1] })).toBe(
      '[t100] splash @ (221, 505)',
    );
  });

  it('formats a death event', () => {
    expect(formatEvent(7, { type: 'death', ship_id: 's_2' })).toBe('[t7] s_2 destroyed');
  });

  it('returns null for unknown event types (forward-compatible)', () => {
    // Cast through unknown to bypass TS narrowing — simulates a future server event the
    // spectator doesn't yet recognise.
    const unknown = { type: 'mystery_buff', amount: 1 } as unknown as Parameters<
      typeof formatEvent
    >[1];
    expect(formatEvent(1, unknown)).toBeNull();
  });

  it('returns null for malformed input', () => {
    expect(formatEvent(1, null)).toBeNull();
    expect(formatEvent(1, undefined)).toBeNull();
    expect(formatEvent(1, {} as unknown as Parameters<typeof formatEvent>[1])).toBeNull();
  });
});
