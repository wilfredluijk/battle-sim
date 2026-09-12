import { COLOR_PALETTE } from './constants';

/** Allocate distinct session colours until the palette is exhausted, then reuse colours.
 * The shared cache keeps every surface consistent; ship identifiers also distinguish teams. */
export function colorFor(name: string, cache: Map<string, string> = defaultCache): string {
  const cached = cache.get(name);
  if (cached) return cached;
  // Simple deterministic string hash (djb2-ish), folded to a palette index.
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = (hash * 31 + name.charCodeAt(i)) | 0;
  }
  const used = new Set(cache.values());
  const preferred = Math.abs(hash) % COLOR_PALETTE.length;
  const color = Array.from({ length: COLOR_PALETTE.length }, (_, i) =>
    COLOR_PALETTE[(preferred + i) % COLOR_PALETTE.length]).find(c => !used.has(c))
    ?? COLOR_PALETTE[preferred];
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
