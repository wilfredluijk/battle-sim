/** Pick a CSS colour for an HP meter based on remaining hull. */
export function hpColor(hp: number): string {
  if (hp > 60) return 'var(--good)';
  if (hp > 25) return '#f4d35e';
  return 'var(--bad)';
}
