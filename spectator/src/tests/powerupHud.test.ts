import { describe, expect, it } from 'vitest';
import { chipsForShip, powerupLabel } from '../lib/powerupHud';
import type { ShipSnapshot } from '../types/protocol';

function ship(overrides: Partial<ShipSnapshot> = {}): ShipSnapshot {
  return {
    id: 's_1',
    bot_name: 'kirk',
    pos: [500, 500],
    heading_deg: 0,
    speed: 0,
    hp: 100,
    ammo: 250,
    throttle: 0,
    rudder: 0,
    alive: true,
    ready: true,
    commands_per_sec: 0,
    sensor_mode: 'active',
    ...overrides,
  };
}

describe('chipsForShip', () => {
  it('returns an empty list when no loadout was picked', () => {
    expect(chipsForShip(ship())).toEqual([]);
  });

  it('renders a ready chip for an unused, inactive powerup', () => {
    const chips = chipsForShip(
      ship({
        selected_powerups: ['overdrive'],
        powerup_status: [{ id: 'overdrive', used: false, active_ticks_left: 0 }],
      }),
    );
    expect(chips).toEqual([
      { id: 'overdrive', label: 'Overdrive', state: 'ready', activeTicksLeft: 0 },
    ]);
  });

  it('renders an active chip with remaining ticks while the effect is live', () => {
    const chips = chipsForShip(
      ship({
        selected_powerups: ['rapid_fire'],
        powerup_status: [{ id: 'rapid_fire', used: true, active_ticks_left: 12 }],
      }),
    );
    expect(chips[0]).toMatchObject({ state: 'active', activeTicksLeft: 12 });
  });

  it('renders a used chip once the effect is gone', () => {
    const chips = chipsForShip(
      ship({
        selected_powerups: ['rapid_fire'],
        powerup_status: [{ id: 'rapid_fire', used: true, active_ticks_left: 0 }],
      }),
    );
    expect(chips[0]).toMatchObject({ state: 'used', activeTicksLeft: 0 });
  });

  it('preserves pick order in the resulting chip strip', () => {
    const chips = chipsForShip(
      ship({
        selected_powerups: ['heavy_shell', 'overdrive'],
        powerup_status: [
          { id: 'heavy_shell', used: false, active_ticks_left: 0 },
          { id: 'overdrive', used: true, active_ticks_left: 0 },
        ],
      }),
    );
    expect(chips.map((c) => c.id)).toEqual(['heavy_shell', 'overdrive']);
  });

  it('falls back to ready when status is missing for a pick', () => {
    const chips = chipsForShip(
      ship({
        selected_powerups: ['overdrive'],
        powerup_status: [],
      }),
    );
    expect(chips[0].state).toBe('ready');
  });
});

describe('powerupLabel', () => {
  it('returns a human-friendly label for a known id', () => {
    expect(powerupLabel('rapid_fire')).toBe('Rapid Fire');
  });

  it('returns the raw id when no label is registered (forward-compat)', () => {
    expect(powerupLabel('warp_drive')).toBe('warp_drive');
  });
});
