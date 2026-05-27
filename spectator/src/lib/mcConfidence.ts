// Wilson score interval for binomial proportions.
//
// Used by the Monte Carlo results panel to put an honest ± on a bot's win rate:
//   "chaser  47/100 wins, 95% CI 37–57%"
//
// We pick Wilson over the naive ±1.96·√(p(1-p)/n) because the naive interval misbehaves
// when `p` is close to 0 or 1 (it can extend past [0, 1] and gets unrealistically tight
// at the boundaries). Wilson stays inside [0, 1] for all (k, n) with n > 0 and matches
// the naive interval when `np`, `n(1-p)` are large.

export interface ConfidenceInterval {
  /** Point estimate `k / n`. Equals 0 when `n` is 0. */
  point: number;
  /** Lower bound of the interval, clamped to `[0, 1]`. */
  lower: number;
  /** Upper bound of the interval, clamped to `[0, 1]`. */
  upper: number;
}

/**
 * Wilson score interval at the requested confidence level.
 *
 * @param wins  successful trials (e.g. matches a bot won)
 * @param total total trials (e.g. matches played)
 * @param confidence  confidence level in `(0, 1)`, default 0.95
 */
export function wilsonInterval(
  wins: number,
  total: number,
  confidence = 0.95,
): ConfidenceInterval {
  if (!Number.isFinite(wins) || !Number.isFinite(total) || total <= 0) {
    return { point: 0, lower: 0, upper: 0 };
  }
  const k = Math.max(0, Math.min(wins, total));
  const n = total;
  const p = k / n;
  const z = normalQuantile(0.5 + confidence / 2);
  const z2 = z * z;
  const denom = 1 + z2 / n;
  const center = (p + z2 / (2 * n)) / denom;
  const halfWidth =
    (z * Math.sqrt((p * (1 - p)) / n + z2 / (4 * n * n))) / denom;
  return {
    point: p,
    lower: clamp01(center - halfWidth),
    upper: clamp01(center + halfWidth),
  };
}

function clamp01(x: number): number {
  if (x < 0) return 0;
  if (x > 1) return 1;
  return x;
}

/**
 * Inverse standard normal CDF via the Beasley-Springer-Moro / Acklam approximation.
 * Accurate to ~1e-9 across the (0, 1) input range, more than enough for confidence
 * intervals where the z-value gets rounded for display anyway.
 *
 * Returns ±∞ for inputs at or beyond `{0, 1}` — callers should pass values in `(0, 1)`.
 */
export function normalQuantile(p: number): number {
  if (p <= 0) return -Infinity;
  if (p >= 1) return Infinity;
  const a = [
    -3.969683028665376e1, 2.209460984245205e2, -2.759285104469687e2,
    1.38357751867269e2, -3.066479806614716e1, 2.506628277459239,
  ];
  const b = [
    -5.447609879822406e1, 1.615858368580409e2, -1.556989798598866e2,
    6.680131188771972e1, -1.328068155288572e1,
  ];
  const c = [
    -7.784894002430293e-3, -3.223964580411365e-1, -2.400758277161838,
    -2.549732539343734, 4.374664141464968, 2.938163982698783,
  ];
  const d = [
    7.784695709041462e-3, 3.224671290700398e-1, 2.445134137142996,
    3.754408661907416,
  ];
  const plow = 0.02425;
  const phigh = 1 - plow;
  let q: number;
  let r: number;
  if (p < plow) {
    q = Math.sqrt(-2 * Math.log(p));
    return (
      (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) /
      ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1)
    );
  }
  if (p > phigh) {
    q = Math.sqrt(-2 * Math.log(1 - p));
    return (
      -(
        (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q +
          c[5]) /
        ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1)
      )
    );
  }
  q = p - 0.5;
  r = q * q;
  return (
    ((((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) *
      q) /
    (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1)
  );
}
