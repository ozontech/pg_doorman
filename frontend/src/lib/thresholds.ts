import type { AuthQueryDto, OverviewDto, PoolDto, Severity } from "../types";

export interface PoolEvaluation {
  poolId: string;
  severity: Severity;
  reasons: string[];
}

export interface HealthState {
  state: Severity;
  reason: string | null;
  perPool: PoolEvaluation[];
  authQuery?: GlobalEvaluation;
}

export interface GlobalEvaluation {
  severity: Severity;
  reasons: string[];
}

export interface PoolHistoryPoint {
  ts: number;
  errors_total: number;
  queries_total: number;
  // Cumulative `pool_scaling.creates` for the pool (reconnect rate input).
  creates_total?: number;
  // Cumulative `pool_scaling.gate_budget_ex` for the pool.
  gate_budget_ex_total?: number;
  // Cumulative `pool_coordinator.exhaustions` for the pool's database.
  // Pools sharing a database see the same series.
  coordinator_exhaustions_total?: number;
}

export type PoolHistory = Map<string, PoolHistoryPoint[]>;

const SUSTAIN_30S_POINTS = 20; // 20 × 1.5 s = 30 s rolling window per parent spec §10.2.
const SUSTAIN_60S_POINTS = 40; // 40 × 1.5 s = 60 s rolling window for slower-cadence rules.
const SUSTAIN_10S_POINTS = 7; // 7 × 1.5 s = 10.5 s — smallest window covering 10 s sustain.
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
// Spec §15.4: per-pool waiting thresholds.
const WAITING_WARN_COUNT = 1;
const WAITING_CRIT_COUNT_FLOOR = 10; // applied as max(10, 0.10×max_connections).
// Spec §15.4: reconnect rate per pool, scaled to the pool's max_connections.
const RECONNECT_WARN_FACTOR = 0.1; // ≥ 0.10 × max_connections / s
const RECONNECT_CRIT_FACTOR = 0.3; // ≥ 0.30 × max_connections / s
// Spec §15.4: per-pool burst-gate budget exhaustion rate.
const GATE_BUDGET_EX_WARN = 0; // > 0 / s sustained
const GATE_BUDGET_EX_CRIT = 0.1; // > 0.1 / s sustained
// Spec §15.4: per-database coordinator exhaustion rate.
const COORD_EXHAUSTIONS_WARN = 0; // > 0 / s sustained
const COORD_EXHAUSTIONS_CRIT = 1.0; // > 1 / s sustained
// Spec §15.4: per-database auth-failure rate (instantaneous ratio over total).
const AUTH_FAIL_WARN_RATIO = 0.005; // 0.5 %
const AUTH_FAIL_CRIT_RATIO = 0.05; // 5 %
// Below this many attempts the failure ratio is dominated by single-event
// noise; the rule stays silent until enough samples accumulate.
const AUTH_MIN_ATTEMPTS = 100;

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

function ratePerSecond(
  history: PoolHistoryPoint[],
  field: (p: PoolHistoryPoint) => number | undefined,
): number | null {
  if (history.length < 2) return null;
  const first = history[0];
  const last = history[history.length - 1];
  const dt = (last.ts - first.ts) / 1000;
  if (dt <= 0) return null;
  const f = field(first);
  const l = field(last);
  if (f === undefined || l === undefined) return null;
  return Math.max(0, (l - f) / dt);
}

function errorsPerSecond(history: PoolHistoryPoint[]): number | null {
  return ratePerSecond(history, (p) => p.errors_total);
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

  // pool.waiting — sustained 10 s per spec §15.4.
  const waitingCrit = Math.max(
    WAITING_CRIT_COUNT_FLOOR,
    Math.floor(0.1 * pool.max_connections),
  );
  if (history) {
    if (
      sustainedAbove(history, SUSTAIN_10S_POINTS, () => pool.waiting >= waitingCrit)
    ) {
      note("critical", `waiting ${pool.waiting} ≥ ${waitingCrit}`);
    } else if (
      sustainedAbove(history, SUSTAIN_10S_POINTS, () => pool.waiting >= WAITING_WARN_COUNT)
    ) {
      note("degraded", `waiting ${pool.waiting} ≥ ${WAITING_WARN_COUNT}`);
    }
  }

  // Reconnect rate — pool_scaling.creates delta scaled to max_connections.
  const reconnectPs = history ? ratePerSecond(history, (p) => p.creates_total) : null;
  if (reconnectPs !== null && history && pool.max_connections > 0) {
    const warn = RECONNECT_WARN_FACTOR * pool.max_connections;
    const crit = RECONNECT_CRIT_FACTOR * pool.max_connections;
    if (sustainedAbove(history, SUSTAIN_30S_POINTS, () => reconnectPs >= crit)) {
      note(
        "critical",
        `reconnect ${reconnectPs.toFixed(2)}/s ≥ ${crit.toFixed(2)} (0.30×max_connections)`,
      );
    } else if (sustainedAbove(history, SUSTAIN_30S_POINTS, () => reconnectPs >= warn)) {
      note(
        "degraded",
        `reconnect ${reconnectPs.toFixed(2)}/s ≥ ${warn.toFixed(2)} (0.10×max_connections)`,
      );
    }
  }

  // Burst-gate budget exhaustion rate — pool_scaling.gate_budget_ex delta.
  const gatePs = history ? ratePerSecond(history, (p) => p.gate_budget_ex_total) : null;
  if (gatePs !== null && history) {
    if (sustainedAbove(history, SUSTAIN_60S_POINTS, () => gatePs > GATE_BUDGET_EX_CRIT)) {
      note("critical", `burst-gate budget exhausted ${gatePs.toFixed(2)}/s > 0.1`);
    } else if (
      sustainedAbove(history, SUSTAIN_60S_POINTS, () => gatePs > GATE_BUDGET_EX_WARN)
    ) {
      note("degraded", `burst-gate budget exhausted ${gatePs.toFixed(2)}/s sustained`);
    }
  }

  // Coordinator exhaustions rate — pool_coordinator.exhaustions delta for the
  // pool's database. Pools sharing a database evaluate the same series.
  const coordPs = history
    ? ratePerSecond(history, (p) => p.coordinator_exhaustions_total)
    : null;
  if (coordPs !== null && history) {
    if (
      sustainedAbove(history, SUSTAIN_60S_POINTS, () => coordPs > COORD_EXHAUSTIONS_CRIT)
    ) {
      note("critical", `coordinator exhaustions ${coordPs.toFixed(2)}/s > 1.0`);
    } else if (
      sustainedAbove(history, SUSTAIN_60S_POINTS, () => coordPs > COORD_EXHAUSTIONS_WARN)
    ) {
      note("degraded", `coordinator exhaustions ${coordPs.toFixed(2)}/s sustained`);
    }
  }

  // Backend gaps that block the rest of spec §15.4:
  //   - TLS handshake error rate: only `pg_doorman_server_tls_handshake_errors_total`
  //     in Prometheus today; needs a counter on PoolDto or a /api/tls endpoint.
  //   - Anonymous LRU evictions: only Prometheus
  //     `pg_doorman_clients_prepared_anonymous_evictions_total`; PoolDto needs
  //     a per-pool eviction counter to support the rule.
  //   - Synthetic misses (SQLSTATE 26000): backend does not classify errors yet
  //     (task #4 in handoff — Top-5 errors with SQLSTATE breakdown).
  //   - fallback_active: the pool exposes neither a boolean nor an
  //     "in-fallback-since" timestamp; PoolDto needs the gauge.
  //   - Patroni API: lives in the standalone patroni_proxy binary, no JSON
  //     over /api today.
  //   - Process RSS vs cgroup limit: backend has no cgroup awareness yet.

  return { poolId: pool.id, severity, reasons };
}

/**
 * Evaluate per-database auth-failure rate over `/api/auth_query` snapshots.
 * Returns the worst severity across all databases that already accumulated
 * at least `AUTH_MIN_ATTEMPTS` attempts.
 */
export function evaluateAuthQuery(authQuery: AuthQueryDto | null): GlobalEvaluation {
  if (!authQuery) return { severity: "ok", reasons: [] };
  let severity: Severity = "ok";
  const reasons: string[] = [];
  for (const row of authQuery.pools) {
    const total = row.auth_success + row.auth_failure;
    if (total < AUTH_MIN_ATTEMPTS) continue;
    const ratio = row.auth_failure / total;
    if (ratio > AUTH_FAIL_CRIT_RATIO) {
      severity = maxSeverity(severity, "critical");
      reasons.push(`auth failure ${(ratio * 100).toFixed(1)} % > 5 % on db ${row.database}`);
    } else if (ratio > AUTH_FAIL_WARN_RATIO) {
      severity = maxSeverity(severity, "degraded");
      reasons.push(`auth failure ${(ratio * 100).toFixed(2)} % > 0.5 % on db ${row.database}`);
    }
  }
  return { severity, reasons };
}

export function aggregateHealth(
  _overview: OverviewDto,
  pools: PoolDto[],
  history: PoolHistory,
  authQuery: AuthQueryDto | null = null,
): HealthState {
  const perPool = pools.map((p) => evaluatePool(p, history.get(p.id)));
  const authQ = evaluateAuthQuery(authQuery);
  let state: Severity = "ok";
  let reason: string | null = null;
  for (const e of perPool) {
    if (rank(e.severity) > rank(state)) {
      state = e.severity;
      reason = e.reasons[0] ?? null;
    }
  }
  if (rank(authQ.severity) > rank(state)) {
    state = authQ.severity;
    reason = authQ.reasons[0] ?? null;
  }
  return { state, reason, perPool, authQuery: authQ };
}
