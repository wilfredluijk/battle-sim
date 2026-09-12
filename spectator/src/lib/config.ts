import type { SimConfig } from '../types/protocol';

/** Preserve tuning that is not represented by the flat parameter form. */
export function mergeConfig(current: SimConfig, edits: Record<string, number>): SimConfig {
  return { ...current, ...edits, ...(current.powerups ? { powerups: { ...current.powerups } } : {}) };
}
