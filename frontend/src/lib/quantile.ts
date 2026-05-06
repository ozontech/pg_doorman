// Linear-interpolation quantile over an unsorted numeric array. Used by
// PanelView's summary table — operators reading the panel expect the
// p50/p95/p99 of the visible window without a backend HDR snapshot. The
// implementation is the standard "type 7" (R default), good enough for
// 200-point sparklines.

export function quantile(values: number[], q: number): number | null {
  if (values.length === 0) return null;
  const sorted = values.filter((v) => Number.isFinite(v)).sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  if (sorted.length === 1) return sorted[0];
  const pos = (sorted.length - 1) * q;
  const base = Math.floor(pos);
  const rest = pos - base;
  const next = sorted[base + 1];
  if (next === undefined) return sorted[base];
  return sorted[base] + rest * (next - sorted[base]);
}

export function summaryStats(values: number[]): {
  count: number;
  min: number | null;
  max: number | null;
  avg: number | null;
  p50: number | null;
  p95: number | null;
  p99: number | null;
} {
  const finite = values.filter((v) => Number.isFinite(v));
  if (finite.length === 0) {
    return { count: 0, min: null, max: null, avg: null, p50: null, p95: null, p99: null };
  }
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  for (const v of finite) {
    sum += v;
    if (v < min) min = v;
    if (v > max) max = v;
  }
  return {
    count: finite.length,
    min,
    max,
    avg: sum / finite.length,
    p50: quantile(finite, 0.5),
    p95: quantile(finite, 0.95),
    p99: quantile(finite, 0.99),
  };
}
