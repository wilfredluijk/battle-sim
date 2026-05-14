import { COLOR_PALETTE } from './constants';

/**
 * Memoised name → palette colour. The mapping is stable for the lifetime of the page so a
 * given bot always renders in the same colour. Names are cycled through `COLOR_PALETTE`
 * in first-seen order. An optional `cache` argument is exposed so tests can isolate state.
 */
export function colorFor(name: string, cache: Map<string, string> = defaultCache): string {
  const cached = cache.get(name);
  if (cached) return cached;
  const next = COLOR_PALETTE[cache.size % COLOR_PALETTE.length];
  cache.set(name, next);
  return next;
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
