import { useEffect, useMemo } from "react";
import { apiGet } from "../api";
import { HealthPill } from "../components/HealthPill";
import { Sparkline } from "../components/Sparkline";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useHistory } from "../hooks/useHistory";
import { usePoll } from "../hooks/usePoll";
import {
  aggregateHealth,
  type HealthState,
  type PoolHistory,
  type PoolHistoryPoint,
} from "../lib/thresholds";
import type { OverviewDto, PoolsDto } from "../types";

const POLL_MS = 1500;
const HISTORY_KEY = "overview";

interface OverviewSamplePoint {
  ts: number;
  query_p95_max_ms: number;
  qps: number;
  tps: number;
  errors_per_s: number;
  saturation_max_pct: number;
}

interface RawTotals {
  ts: number;
  query_count_total: number;
  transaction_count_total: number;
  errors_count_total: number;
}

type PoolSnap = Record<string, PoolHistoryPoint>;

const EMPTY_HEALTH: HealthState = { state: "ok", reason: null, perPool: [] };

export default function Overview() {
  const { authHeader } = useAdminAuth();
  const overviewPoll = usePoll<OverviewDto>(
    (signal) => apiGet<OverviewDto>("/api/overview", authHeader, signal),
    POLL_MS,
  );
  const poolsPoll = usePoll<PoolsDto>(
    (signal) => apiGet<PoolsDto>("/api/pools", authHeader, signal),
    POLL_MS,
  );

  const rawHistory = useHistory<RawTotals>(`${HISTORY_KEY}.raw`);
  const sampleHistory = useHistory<OverviewSamplePoint>(HISTORY_KEY);
  const poolErrorsHistory = useHistory<PoolSnap>(`${HISTORY_KEY}.poolerrs`);

  useEffect(() => {
    if (!overviewPoll.data || !poolsPoll.data) return;
    const ov = overviewPoll.data;
    const pools = poolsPoll.data;
    const prevRaw = rawHistory.history[rawHistory.history.length - 1];
    rawHistory.push({
      ts: ov.ts,
      query_count_total: ov.query_count_total,
      transaction_count_total: ov.transaction_count_total,
      errors_count_total: ov.errors_count_total,
    });
    let qps = 0;
    let tps = 0;
    let errPs = 0;
    if (prevRaw) {
      const dt = (ov.ts - prevRaw.ts) / 1000;
      if (dt > 0) {
        qps = Math.max(0, (ov.query_count_total - prevRaw.query_count_total) / dt);
        tps = Math.max(0, (ov.transaction_count_total - prevRaw.transaction_count_total) / dt);
        errPs = Math.max(0, (ov.errors_count_total - prevRaw.errors_count_total) / dt);
      }
    }
    sampleHistory.push({
      ts: ov.ts,
      query_p95_max_ms: pools.pools.reduce((m, p) => Math.max(m, p.query_p95_ms), 0),
      qps,
      tps,
      errors_per_s: errPs,
      saturation_max_pct:
        pools.pools.reduce((m, p) => {
          const s = p.max_connections > 0 ? p.connections / p.max_connections : 0;
          return Math.max(m, s);
        }, 0) * 100,
    });
    const snap: PoolSnap = {};
    for (const p of pools.pools) {
      snap[p.id] = { ts: ov.ts, errors_total: p.errors_total, queries_total: p.queries_total };
    }
    poolErrorsHistory.push(snap);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overviewPoll.data?.ts, poolsPoll.data?.ts]);

  const poolHistoryForEngine: PoolHistory = useMemo(() => {
    const map: PoolHistory = new Map();
    for (const snap of poolErrorsHistory.history) {
      for (const id of Object.keys(snap)) {
        const list = map.get(id) ?? [];
        list.push(snap[id]);
        map.set(id, list);
      }
    }
    return map;
  }, [poolErrorsHistory.history]);

  const health = useMemo(() => {
    if (!overviewPoll.data || !poolsPoll.data) return EMPTY_HEALTH;
    return aggregateHealth(overviewPoll.data, poolsPoll.data.pools, poolHistoryForEngine);
  }, [overviewPoll.data, poolsPoll.data, poolHistoryForEngine]);

  const seriesXs = useMemo(
    () => sampleHistory.history.map((s) => s.ts / 1000),
    [sampleHistory.history],
  );

  const sigSeries = (extract: (s: OverviewSamplePoint) => number): [number[], number[]] => [
    seriesXs,
    sampleHistory.history.map(extract),
  ];

  const latest = sampleHistory.history[sampleHistory.history.length - 1];

  const fmtMs = (n: number | undefined) => (n === undefined ? "—" : `${Math.round(n)} ms`);
  const fmtRate = (n: number | undefined, suffix: string) =>
    n === undefined ? "—" : `${n.toFixed(n < 10 ? 2 : 0)} ${suffix}`;
  const fmtPct = (n: number | undefined) => (n === undefined ? "—" : `${Math.round(n)}%`);

  if (overviewPoll.error || poolsPoll.error) {
    const err = overviewPoll.error?.message ?? poolsPoll.error?.message ?? "fetch failed";
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Overview</h1>
        <p className="mt-2 text-sm text-danger">{err}</p>
      </section>
    );
  }

  return (
    <div className="flex flex-col">
      <HealthPill health={health} lastUpdated={overviewPoll.lastUpdated} />
      <div className="grid grid-cols-4 border-b border-border">
        <Sparkline
          label="Latency P95"
          valueText={fmtMs(latest?.query_p95_max_ms)}
          series={sigSeries((s) => s.query_p95_max_ms)}
          warn={100}
          crit={500}
          logY
          syncKey="overview"
        />
        <Sparkline
          label="Traffic"
          valueText={`${fmtRate(latest?.qps, "qps")} / ${fmtRate(latest?.tps, "tps")}`}
          series={sigSeries((s) => s.qps)}
          syncKey="overview"
        />
        <Sparkline
          label="Errors/s"
          valueText={fmtRate(latest?.errors_per_s, "/s")}
          series={sigSeries((s) => s.errors_per_s)}
          warn={1}
          crit={10}
          syncKey="overview"
        />
        <Sparkline
          label="Saturation max"
          valueText={fmtPct(latest?.saturation_max_pct)}
          series={sigSeries((s) => s.saturation_max_pct)}
          warn={70}
          crit={90}
          syncKey="overview"
        />
      </div>
      <p className="px-4 py-3 text-xs text-text-dim">
        Phase 6a MVP: Health bar + Golden Signals only. Connection breakdown,
        Pool heatmap, dual-axis wait + oldest-active-age, top-5 errors, and
        resource detail land in 6a-2.
      </p>
    </div>
  );
}
