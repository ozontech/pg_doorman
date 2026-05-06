import { useEffect, useMemo } from "react";
import { apiGet } from "../api";
import { AreaChart } from "../components/AreaChart";
import { DualAxisChart } from "../components/DualAxisChart";
import { HealthPill } from "../components/HealthPill";
import { Heatmap } from "../components/Heatmap";
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
const HEATMAP_CELLS = 60;

interface OverviewSamplePoint {
  ts: number;
  query_p95_max_ms: number;
  qps: number;
  tps: number;
  errors_per_s: number;
  saturation_max_pct: number;
  active_clients: number;
  idle_clients: number;
  waiting_clients: number;
  oldest_active_age_max_ms: number;
}

interface RawTotals {
  ts: number;
  query_count_total: number;
  transaction_count_total: number;
  errors_count_total: number;
}

type PoolSnap = Record<string, PoolHistoryPoint>;
type PoolSatSnap = Record<string, { saturation: number; max_connections: number; label: string }>;

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
  const poolSatHistory = useHistory<PoolSatSnap>(`${HISTORY_KEY}.poolsat`);

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
      active_clients: ov.active_clients,
      idle_clients: ov.idle_clients,
      waiting_clients: ov.waiting_clients,
      oldest_active_age_max_ms: pools.pools.reduce(
        (m, p) => Math.max(m, p.max_active_age_ms),
        0,
      ),
    });
    const errSnap: PoolSnap = {};
    const satSnap: PoolSatSnap = {};
    for (const p of pools.pools) {
      errSnap[p.id] = { ts: ov.ts, errors_total: p.errors_total, queries_total: p.queries_total };
      satSnap[p.id] = {
        saturation: p.max_connections > 0 ? p.connections / p.max_connections : 0,
        max_connections: p.max_connections,
        label: p.id,
      };
    }
    poolErrorsHistory.push(errSnap);
    poolSatHistory.push(satSnap);
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

  // Connection breakdown: stacked area active / idle / waiting over the
  // sample window. Three separate (non-cumulative) series; AreaChart stacks
  // them internally.
  const connBreakdown: [number[], ...number[][]] = useMemo(() => {
    const xs = sampleHistory.history.map((s) => s.ts / 1000);
    const active = sampleHistory.history.map((s) => s.active_clients);
    const idle = sampleHistory.history.map((s) => s.idle_clients);
    const waiting = sampleHistory.history.map((s) => s.waiting_clients);
    return [xs, active, idle, waiting];
  }, [sampleHistory.history]);

  // Top 5 pools by errors-per-second over the last 30 s window. We compute
  // each pool's eps by walking poolErrorsHistory and taking the average over
  // the last SUSTAIN points; the resulting AreaChart paints the last sample
  // window with each pool's eps as a stacked band.
  const top5Errors = useMemo(() => {
    const ids = poolsPoll.data ? poolsPoll.data.pools.map((p) => p.id) : [];
    const recent = poolErrorsHistory.history.slice(-20);
    const epsById = new Map<string, number>();
    for (const id of ids) {
      let prev: PoolHistoryPoint | undefined;
      let max = 0;
      for (const snap of recent) {
        const cur = snap[id];
        if (!cur) continue;
        if (prev) {
          const dt = (cur.ts - prev.ts) / 1000;
          if (dt > 0) {
            const eps = Math.max(0, (cur.errors_total - prev.errors_total) / dt);
            if (eps > max) max = eps;
          }
        }
        prev = cur;
      }
      epsById.set(id, max);
    }
    const top = ids
      .map((id) => ({ id, eps: epsById.get(id) ?? 0 }))
      .filter((x) => x.eps > 0)
      .sort((a, b) => b.eps - a.eps)
      .slice(0, 5);
    if (top.length === 0) return { labels: [] as string[], data: [[]] as [number[], ...number[][]] };
    const xs = poolErrorsHistory.history.map((snap) => {
      const anyKey = top.find((t) => snap[t.id])?.id;
      return anyKey ? snap[anyKey].ts / 1000 : 0;
    });
    const series = top.map(({ id }) => {
      const out: number[] = [];
      let prev: PoolHistoryPoint | undefined;
      for (const snap of poolErrorsHistory.history) {
        const cur = snap[id];
        if (!cur || !prev) {
          out.push(0);
          prev = cur ?? prev;
          continue;
        }
        const dt = (cur.ts - prev.ts) / 1000;
        const eps = dt > 0 ? Math.max(0, (cur.errors_total - prev.errors_total) / dt) : 0;
        out.push(eps);
        prev = cur;
      }
      return out;
    });
    return {
      labels: top.map((t) => t.id),
      data: [xs, ...series] as [number[], ...number[][]],
    };
  }, [poolErrorsHistory.history, poolsPoll.data]);

  // Pool fill heatmap rows: one per current pool, last HEATMAP_CELLS cells of
  // saturation. Pads with `null` on the left when history is shorter.
  const heatmapRows = useMemo(() => {
    const ids = poolsPoll.data ? poolsPoll.data.pools.map((p) => p.id) : [];
    const capacities = new Map<string, number>();
    if (poolsPoll.data) {
      for (const p of poolsPoll.data.pools) capacities.set(p.id, p.max_connections);
    }
    const recent = poolSatHistory.history.slice(-HEATMAP_CELLS);
    return ids.map((id) => {
      const cells: (number | null)[] = new Array(HEATMAP_CELLS).fill(null);
      const offset = HEATMAP_CELLS - recent.length;
      for (let i = 0; i < recent.length; i++) {
        const cell = recent[i][id];
        cells[offset + i] = cell ? cell.saturation : null;
      }
      return { label: id, cells, capacity: capacities.get(id) ?? 0 };
    });
  }, [poolsPoll.data, poolSatHistory.history]);

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
      <section className="border-b border-border">
        <div className="px-4 py-2 text-xs text-text-muted uppercase tracking-wide">
          Connection breakdown — active / idle / waiting (last 3 min)
        </div>
        <AreaChart
          data={connBreakdown}
          labels={["active", "idle", "waiting"]}
          fills={["rgb(45 194 107 / 0.5)", "rgb(138 147 164 / 0.4)", "rgb(245 165 36 / 0.5)"]}
          syncKey="overview"
        />
      </section>
      {heatmapRows.length > 0 && <Heatmap rows={heatmapRows} />}
      <section className="border-b border-border">
        <div className="px-4 py-2 text-xs text-text-muted uppercase tracking-wide">
          Wait queue (left) vs oldest-active-age ms (right, log)
        </div>
        <DualAxisChart
          data={[
            seriesXs,
            sampleHistory.history.map((s) => s.waiting_clients),
            sampleHistory.history.map((s) => Math.max(1, s.oldest_active_age_max_ms)),
          ]}
          leftLabel="waiting"
          rightLabel="oldest-active ms"
          leftStroke="rgb(91 140 255)"
          rightStroke="rgb(245 165 36)"
          rightLogScale
          rightWarn={30_000}
          rightCrit={300_000}
          syncKey="overview"
        />
      </section>
      {top5Errors.labels.length > 0 && (
        <section className="border-b border-border">
          <div className="px-4 py-2 text-xs text-text-muted uppercase tracking-wide">
            Top {top5Errors.labels.length} pools by error rate (last 30 s)
          </div>
          <AreaChart
            data={top5Errors.data}
            labels={top5Errors.labels}
            fills={[
              "rgb(229 72 77 / 0.55)",
              "rgb(245 165 36 / 0.55)",
              "rgb(177 140 245 / 0.55)",
              "rgb(91 140 255 / 0.5)",
              "rgb(45 194 107 / 0.45)",
            ]}
            syncKey="overview"
          />
        </section>
      )}
      <p className="px-4 py-3 text-xs text-text-dim">
        Phase 6a-3 done. Resource detail (memory/sockets/interner) lands in
        6a-4 once the polled endpoints are wired into a collapsed section.
      </p>
    </div>
  );
}
