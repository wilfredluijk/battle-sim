// Pure helpers for rendering the per-ship powerup HUD.
//
// These don't touch DOM or canvas — they translate a `ShipSnapshot`'s loadout state
// into chip data that components can render however they like. Unit-testable with
// Vitest (see `tests/powerupHud.test.ts`).

import type { PowerupStatus, ShipSnapshot } from '../types/protocol';

export type ChipState = 'ready' | 'active' | 'used';

/** One chip in a ship's loadout strip. */
export interface PowerupChip {
  /** Wire id, e.g. `"rapid_fire"`. */
  id: string;
  /** Human-friendly label suitable for a short tooltip / chip caption. */
  label: string;
  state: ChipState;
  /** Ticks of effect remaining when `state === 'active'`; `0` otherwise. */
  activeTicksLeft: number;
}

const LABELS: Record<string, string> = {
  overdrive: 'Overdrive',
  reinforced_hull: 'Reinforced Hull',
  repair_drones: 'Repair Drones',
  smoke_screen: 'Smoke Screen',
  rapid_fire: 'Rapid Fire',
  heavy_shell: 'Heavy Shell',
  long_range_salvo: 'Long-Range Salvo',
  awacs_scan: 'AWACS Scan',
  silent_running: 'Silent Running',
  counter_battery_trace: 'Counter-Battery Trace',
  emp_burst: 'EMP Burst',
  decoy_flare: 'Decoy Flare',
};

/** Look up a friendly label for a powerup id, falling back to the id itself. */
export function powerupLabel(id: string): string {
  return LABELS[id] ?? id;
}

/**
 * Build the chip strip for a ship. Returns an empty array when the bot picked nothing.
 * Order matches the bot's pick order so the HUD layout is stable across ticks.
 */
export function chipsForShip(ship: ShipSnapshot): PowerupChip[] {
  const picks = ship.selected_powerups ?? [];
  if (picks.length === 0) return [];
  const statusById = new Map<string, PowerupStatus>(
    (ship.powerup_status ?? []).map((s) => [s.id, s]),
  );
  return picks.map((id) => {
    const status = statusById.get(id);
    const active = (status?.active_ticks_left ?? 0) > 0;
    const used = status?.used ?? false;
    // A powerup with `used=true && active_ticks_left>0` is currently in effect — show
    // it as "active" so the operator can see the buff burning down.
    const state: ChipState = active ? 'active' : used ? 'used' : 'ready';
    return {
      id,
      label: powerupLabel(id),
      state,
      activeTicksLeft: status?.active_ticks_left ?? 0,
    };
  });
}
