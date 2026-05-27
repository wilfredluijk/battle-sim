import { describe, expect, it } from 'vitest';
import { normalQuantile, wilsonInterval } from '../lib/mcConfidence';

describe('normalQuantile', () => {
  it('matches well-known z-values at the standard confidence levels', () => {
    // Two-sided 95% → upper tail at p = 0.975 → z ≈ 1.96.
    expect(normalQuantile(0.975)).toBeCloseTo(1.96, 2);
    // 90% → 0.95 → 1.645.
    expect(normalQuantile(0.95)).toBeCloseTo(1.645, 2);
    // 99% → 0.995 → 2.576.
    expect(normalQuantile(0.995)).toBeCloseTo(2.576, 2);
  });
  it('returns ±infinity at the boundary', () => {
    expect(normalQuantile(0)).toBe(-Infinity);
    expect(normalQuantile(1)).toBe(Infinity);
  });
});

describe('wilsonInterval', () => {
  it('returns the point estimate inside the interval', () => {
    const ci = wilsonInterval(47, 100);
    expect(ci.point).toBeCloseTo(0.47, 6);
    expect(ci.lower).toBeLessThanOrEqual(ci.point);
    expect(ci.upper).toBeGreaterThanOrEqual(ci.point);
  });
  it('keeps the interval inside [0, 1]', () => {
    // Extreme tails: 0/10 wins or 10/10 wins should not extend past the unit interval.
    const lo = wilsonInterval(0, 10);
    expect(lo.lower).toBe(0);
    expect(lo.upper).toBeLessThanOrEqual(1);
    expect(lo.upper).toBeGreaterThan(0);
    const hi = wilsonInterval(10, 10);
    expect(hi.upper).toBe(1);
    expect(hi.lower).toBeGreaterThanOrEqual(0);
    expect(hi.lower).toBeLessThan(1);
  });
  it('matches the textbook value for 50/100 @ 95%', () => {
    // Wilson at k=50, n=100, conf=95% → roughly [0.40, 0.60].
    const ci = wilsonInterval(50, 100);
    expect(ci.lower).toBeCloseTo(0.4038, 3);
    expect(ci.upper).toBeCloseTo(0.5962, 3);
  });
  it('returns a zero interval for zero trials', () => {
    expect(wilsonInterval(0, 0)).toEqual({ point: 0, lower: 0, upper: 0 });
    expect(wilsonInterval(5, 0)).toEqual({ point: 0, lower: 0, upper: 0 });
  });
  it('clamps wins to total to handle bad inputs', () => {
    const ci = wilsonInterval(100, 50);
    expect(ci.point).toBe(1);
  });
});
