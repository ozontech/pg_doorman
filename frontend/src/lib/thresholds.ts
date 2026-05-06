import type { OverviewDto, PoolDto, Severity } from "../types";

export interface PoolEvaluation {
  poolId: string;
  severity: Severity;
  reasons: string[];
}

export interface HealthState {
  state: Severity;
  reason: string | null;
  perPool: PoolEvaluation[];
}

export interface PoolHistoryPoint {
  ts: number;
  errors_total: number;
  queries_total: number;
}

export type PoolHistory = Map<string, PoolHistoryPoint[]>;

const SUSTAIN_30S_POINTS = 20; // 20 × 1.5 s = 30 s rolling window per parent spec §10.2.
const ERRORS_PER_SEC_WARN = 0.1;
const ERRORS_PER_SEC_CRIT = 1.0;
const SATURATION_WARN = 0.7;
const SATURATION_CRIT = 0.9;
const QUERY_P95_WARN_MS = 100;
const QUERY_P95_CRIT_MS = 500;
const QUERY_P99_WARN_MS = 500;
const QUERY_P99_CRIT_MS = 2000;
const ACTIVE_AGE_WARN_MS = 30_000;
const ACTIVE_AGE_CRIT_MS = 300_000;
const WAIT_AVG_WARN_MS = 5;
const WAIT_AVG_CRIT_MS = 50;
const WAIT_P95_WARN_MS = 50;
const WAIT_P95_CRIT_MS = 500;

function rank(s: Severity): number {
  switch (s) {
    case "ok":
      return 0;
    case "degraded":
      return 1;
    case "critical":
      return 2;
  }
}

function maxSeverity(a: Severity, b: Severity): Severity {
  return rank(a) >= rank(b) ? a : b;
}

function errorsPerSecond(history: PoolHistoryPoint[]): number | null {
  if (history.length < 2) return null;
  const first = history[0];
  const last = history[history.length - 1];
  const dt = (last.ts - first.ts) / 1000;
  if (dt <= 0) return null;
  return Math.max(0, (last.errors_total - first.errors_total) / dt);
}

function sustainedAbove(
  history: PoolHistoryPoint[],
  points: number,
  predicate: (p: PoolHistoryPoint) => boolean,
): boolean {
  if (history.length < points) return false;
  return history.slice(-points).every(predicate);
}

export function evaluatePool(
  pool: PoolDto,
  history: PoolHistoryPoint[] | undefined,
): PoolEvaluation {
  let severity: Severity = "ok";
  const reasons: string[] = [];
  const note = (lvl: Severity, msg: string) => {
    severity = maxSeverity(severity, lvl);
    reasons.push(msg);
  };

  if (pool.max_connections > 0) {
    const sat = pool.connections / pool.max_connections;
    if (sat >= SATURATION_CRIT) note("critical", `saturation ${(sat * 100).toFixed(0)}% ≥ 90%`);
    else if (sat >= SATURATION_WARN) note("degraded", `saturation ${(sat * 100).toFixed(0)}% ≥ 70%`);
  }

  if (pool.max_active_age_ms > ACTIVE_AGE_CRIT_MS) {
    note("critical", `oldest-active ${pool.max_active_age_ms} ms > 300 s`);
  } else if (pool.max_active_age_ms > ACTIVE_AGE_WARN_MS) {
    note("degraded", `oldest-active ${pool.max_active_age_ms} ms > 30 s`);
  }

  if (pool.query_p95_ms > QUERY_P95_CRIT_MS)
    note("critical", `p95 ${pool.query_p95_ms} ms > 500`);
  else if (pool.query_p95_ms > QUERY_P95_WARN_MS)
    note("degraded", `p95 ${pool.query_p95_ms} ms > 100`);
  if (pool.query_p99_ms > QUERY_P99_CRIT_MS)
    note("critical", `p99 ${pool.query_p99_ms} ms > 2000`);
  else if (pool.query_p99_ms > QUERY_P99_WARN_MS)
    note("degraded", `p99 ${pool.query_p99_ms} ms > 500`);

  if (pool.wait_avg_ms > WAIT_AVG_CRIT_MS)
    note("critical", `wait avg ${pool.wait_avg_ms} ms > 50`);
  else if (pool.wait_avg_ms > WAIT_AVG_WARN_MS)
    note("degraded", `wait avg ${pool.wait_avg_ms} ms > 5`);
  if (pool.wait_p95_ms > WAIT_P95_CRIT_MS)
    note("critical", `wait p95 ${pool.wait_p95_ms} ms > 500`);
  else if (pool.wait_p95_ms > WAIT_P95_WARN_MS)
    note("degraded", `wait p95 ${pool.wait_p95_ms} ms > 50`);

  const eps = history ? errorsPerSecond(history) : null;
  if (eps !== null && history) {
    if (sustainedAbove(history, SUSTAIN_30S_POINTS, () => eps > ERRORS_PER_SEC_CRIT)) {
      note("critical", `errors ${eps.toFixed(2)}/s > 1.0 sustained`);
    } else if (sustainedAbove(history, SUSTAIN_30S_POINTS, () => eps > ERRORS_PER_SEC_WARN)) {
      note("degraded", `errors ${eps.toFixed(2)}/s > 0.1 sustained`);
    }
  }

  // TODO(phase 6b): auth-failure rate, TLS handshake errors, anonymous LRU
  // evictions, Patroni API health — none exposed on PoolDto today.

  return { poolId: pool.id, severity, reasons };
}

export function aggregateHealth(
  _overview: OverviewDto,
  pools: PoolDto[],
  history: PoolHistory,
): HealthState {
  const perPool = pools.map((p) => evaluatePool(p, history.get(p.id)));
  let state: Severity = "ok";
  let reason: string | null = null;
  for (const e of perPool) {
    if (rank(e.severity) > rank(state)) {
      state = e.severity;
      reason = e.reasons[0] ?? null;
    }
  }
  return { state, reason, perPool };
}
