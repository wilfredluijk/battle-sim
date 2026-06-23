import { COLOR_PALETTE } from './constants';

/**
 * Name → palette colour, derived by hashing the name into `COLOR_PALETTE`. The mapping is
 * deterministic, so a given bot renders in the same colour on every surface (sidebar,
 * report, canvas) regardless of the order names are first seen — unlike a first-seen counter,
 * which could assign different colours to the same bot on different screens. The `cache` just
 * memoises the hash; an optional argument is exposed so tests can isolate state.
 */
export function colorFor(name: string, cache: Map<string, string> = defaultCache): string {
  const cached = cache.get(name);
  if (cached) return cached;
  // Simple deterministic string hash (djb2-ish), folded to a palette index.
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = (hash * 31 + name.charCodeAt(i)) | 0;
  }
  const color = COLOR_PALETTE[Math.abs(hash) % COLOR_PALETTE.length];
  cache.set(name, color);
  return color;
}

const defaultCache = new Map<string, string>();

/** Convert `#rrggbb` (or `#rgb`) to `rgba(r, g, b, a)` with the given alpha. */
export function withAlpha(hex: string, a: number): string {
  let h = hex.replace('#', '');
  if (h.length === 3) {
    h = h
      .split('')
      .map((c) => c + c)
      .join('');
  }
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${a})`;
}
