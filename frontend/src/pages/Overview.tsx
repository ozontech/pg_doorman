import { useEffect, useMemo, useRef, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { apiGet } from "../api";
import { AreaChart } from "../components/AreaChart";
import { Collapsible } from "../components/Collapsible";
import { DualAxisChart } from "../components/DualAxisChart";
import { Heatmap } from "../components/Heatmap";
import { PageHero } from "../components/PageHero";
import { MemoryPanel } from "../components/MemoryPanel";
import { PanelView, type PanelKind } from "../components/PanelView";
import { SectionHeader } from "../components/SectionHeader";
import { Sparkline } from "../components/Sparkline";
import { describeSqlstate } from "../lib/sqlstate";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useHistory } from "../hooks/useHistory";
import { usePoll } from "../hooks/usePoll";
import type { PoolHistoryPoint } from "../lib/thresholds";
import type { ChartEvent } from "../components/Sparkline";
import type {
  EventsDto,
  InternerDto,
  OverviewDto,
  PoolCoordinatorDto,
  PoolDto,
  PoolScalingDto,
  PoolsDto,
  ProcessDto,
  SocketsDto,
} from "../types";

const POLL_MS = 1500;
const HISTORY_KEY = "overview";
const HEATMAP_CELLS = 60;
// Host-scoped so two pooler tabs keep separate process baselines.
const PREV_PROCESS_KEY = `pgdoorman.prev.process.${
  typeof window !== "undefined" ? window.location.host : "any"
}`;

function loadPrevProcess(): ProcessDto | null {
  try {
    const raw = localStorage.getItem(PREV_PROCESS_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as ProcessDto;
    if (typeof parsed.ts !== "number") return null;
    return parsed;
  } catch {
    return null;
  }
}

function savePrevProcess(v: ProcessDto) {
  try {
    localStorage.setItem(PREV_PROCESS_KEY, JSON.stringify(v));
  } catch {
    /* private mode / quota — no-op. */
  }
}

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
  // Resource detail polls — slower cadence (3 s) since the data is not
  // hot-path and the section is collapsed by default.
  const socketsPoll = usePoll<SocketsDto>(
    (signal) => apiGet<SocketsDto>("/api/sockets", authHeader, signal),
    3000,
  );
  const internerPoll = usePoll<InternerDto>(
    (signal) => apiGet<InternerDto>("/api/interner", authHeader, signal),
    3000,
  );
  // Threshold-only polls for §15.4 reconnect-rate, gate-budget, coordinator,
  // and auth-failure rules. Not rendered, but their counters feed the
  // per-pool history that the threshold engine reads.
  const scalingPoll = usePoll<PoolScalingDto>(
    (signal) => apiGet<PoolScalingDto>("/api/pool_scaling", authHeader, signal),
    POLL_MS,
  );
  const coordPoll = usePoll<PoolCoordinatorDto>(
    (signal) => apiGet<PoolCoordinatorDto>("/api/pool_coordinator", authHeader, signal),
    POLL_MS,
  );
  // Process resources (RSS, CPU, FDs, threads). Slow cadence (3 s) — the
  // process card is informational, not alerting, and these /proc reads are
  // not free at 1.5 s.
  const processPoll = usePoll<ProcessDto>(
    (signal) => apiGet<ProcessDto>("/api/process", authHeader, signal),
    3000,
  );
  // Admin event ring (RELOAD/PAUSE/RESUME/RECONNECT) — paint vertical
  // annotation lines on every chart so a metric spike correlates with the
  // operator action that caused it.
  const eventsPoll = usePoll<EventsDto>(
    (signal) => apiGet<EventsDto>("/api/events", authHeader, signal),
    3000,
  );
  const chartEvents: ChartEvent[] = useMemo(() => {
    const list = eventsPoll.data?.events ?? [];
    return list.map((e) => ({ ts: e.ts_ms / 1000, label: e.target }));
  }, [eventsPoll.data]);

  // Per-thread CPU% history. Each successful /api/process poll computes a
  // delta against the previous snapshot and pushes a row into a rolling
  // 120-point window. The threads PanelView reads this history to render
  // a line per tokio worker so an imbalanced runtime is visible as one
  // line at 100% while the others stay near 0.
  const processSnapshotsRef = useRef<ProcessDto[]>([]);
  const threadHistoryRef = useRef<
    Array<{
      ts: number;
      // pct keyed by tid for the *whole* known set at this snapshot. Threads
      // that disappeared or just appeared are filled with NaN so uPlot
      // breaks the line at the join.
      pcts: Map<number, number>;
      names: Map<number, string>;
    }>
  >([]);
  if (processPoll.data) {
    const snapshots = processSnapshotsRef.current;
    const last = snapshots[snapshots.length - 1];
    if (!last || last.ts !== processPoll.data.ts) {
      const cur = processPoll.data;
      if (last && cur.ts > last.ts) {
        const dtSec = (cur.ts - last.ts) / 1000;
        if (dtSec > 0) {
          const pcts = new Map<number, number>();
          const names = new Map<number, string>();
          const lastByTid = new Map<number, number>(
            last.threads_breakdown.map((t) => [t.tid, t.cpu_user_us + t.cpu_system_us]),
          );
          for (const t of cur.threads_breakdown) {
            const prevTotal = lastByTid.get(t.tid);
            const curTotal = t.cpu_user_us + t.cpu_system_us;
            const pct =
              prevTotal === undefined
                ? 0
                : Math.max(0, ((curTotal - prevTotal) / 1_000_000 / dtSec) * 100);
            pcts.set(t.tid, pct);
            names.set(t.tid, t.name);
          }
          threadHistoryRef.current.push({ ts: cur.ts, pcts, names });
          if (threadHistoryRef.current.length > 240) threadHistoryRef.current.shift();
        }
      }
      snapshots.push(cur);
      if (snapshots.length > 240) snapshots.shift();
    }
  }

  const rawHistory = useHistory<RawTotals>(`${HISTORY_KEY}.raw`);
  const sampleHistory = useHistory<OverviewSamplePoint>(HISTORY_KEY);
  const poolErrorsHistory = useHistory<PoolSnap>(`${HISTORY_KEY}.poolerrs`);
  const poolSatHistory = useHistory<PoolSatSnap>(`${HISTORY_KEY}.poolsat`);

  useEffect(() => {
    if (!overviewPoll.data || !poolsPoll.data) return;
    const ov = overviewPoll.data;
    const pools = poolsPoll.data;
    const prevRaw = rawHistory.history[rawHistory.history.length - 1];
    // Stale-tab guard: if the gap since the last poll is bigger than the
    // window below, the laptop probably slept or the operator left the
    // tab for an extended time — drop the history so the chart does not
    // bridge a multi-hour gap with a flat line. The previous 5 × 1.5 s
    // window (7.5 s) was too aggressive: an operator visiting the war
    // room for ten seconds and coming back saw every sparkline reset
    // to "collecting samples · 1/120". 90 s keeps the safety net for
    // real sleep events while letting brief navigation be invisible.
    if (prevRaw && ov.ts - prevRaw.ts > 90_000) {
      rawHistory.replace([]);
      sampleHistory.replace([]);
      poolErrorsHistory.replace([]);
      poolSatHistory.replace([]);
      return;
    }
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
          const s = p.max_connections > 0 ? p.active / p.max_connections : 0;
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
    // Index sibling-endpoint rows for O(1) lookup per pool id below.
    const scalingByKey = new Map<string, { creates: number; gate_budget_ex: number }>();
    if (scalingPoll.data) {
      for (const r of scalingPoll.data.pools) {
        scalingByKey.set(`${r.user}@${r.database}`, {
          creates: r.creates,
          gate_budget_ex: r.gate_budget_ex,
        });
      }
    }
    const coordByDb = new Map<string, number>();
    if (coordPoll.data) {
      for (const r of coordPoll.data.databases) {
        coordByDb.set(r.database, r.exhaustions);
      }
    }
    for (const p of pools.pools) {
      const scaling = scalingByKey.get(p.id);
      errSnap[p.id] = {
        ts: ov.ts,
        errors_total: p.errors_total,
        queries_total: p.queries_total,
        creates_total: scaling?.creates,
        gate_budget_ex_total: scaling?.gate_budget_ex,
        coordinator_exhaustions_total: coordByDb.get(p.database),
      };
      satSnap[p.id] = {
        saturation: p.max_connections > 0 ? p.active / p.max_connections : 0,
        max_connections: p.max_connections,
        label: p.id,
      };
    }
    poolErrorsHistory.push(errSnap);
    poolSatHistory.push(satSnap);
    // The effect keys on the overview timestamp only. Pools, scaling,
    // coordinator, and auth-query polls all run independently and their
    // timestamps drift relative to the overview cadence — including any of
    // them in the dep array makes the effect fire mid-interval with
    // `dt ≈ 200 ms` and a tiny `delta`, which the sparkline draws as a
    // sawtooth wave dropping to zero between each real overview tick.
    // pools/scaling/coord data is still read snapshot-style at each fire
    // so the per-pool history retains the threshold-engine fields.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overviewPoll.data?.ts]);

  const seriesXs = useMemo(
    () => sampleHistory.history.map((s) => s.ts / 1000),
    [sampleHistory.history],
  );

  const sigSeries = (extract: (s: OverviewSamplePoint) => number): [number[], number[]] => [
    seriesXs,
    sampleHistory.history.map(extract),
  ];

  // Panel drill-down state. The currently-open panel is encoded in the
  // route query string so a deep-link to e.g. ?panel=traffic survives a
  // page reload and can be shared during an incident handover.
  const [searchParams, setSearchParams] = useSearchParams();
  const openPanel = searchParams.get("panel");
  const closePanel = () => {
    const next = new URLSearchParams(searchParams);
    next.delete("panel");
    setSearchParams(next, { replace: true });
  };
  const openPanelById = (id: string) => {
    const next = new URLSearchParams(searchParams);
    next.set("panel", id);
    setSearchParams(next, { replace: false });
  };

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

  // Compact human-friendly duration. Tile widths are tight; the suffix
  // sits right after the number with no space so the whole string fits
  // a 6-character mono cell at any magnitude. "87ms" / "1.2s" / "1m29s"
  // / "1h42m". Operators read this as a number-plus-magnitude word, the
  // exact precision is in the hover tooltip.
  const fmtMs = (n: number | undefined): string => {
    if (n === undefined) return "—";
    // Sub-millisecond values arrive as fractions (e.g. 0.42 ms). Show
    // two decimals so an operator does not see "0ms" for a pool that
    // actually serves 420-microsecond p95.
    if (n > 0 && n < 1) return `${n.toFixed(2)}ms`;
    if (n < 10) return `${n.toFixed(1)}ms`;
    if (n < 1000) return `${Math.round(n)}ms`;
    if (n < 10_000) return `${(n / 1000).toFixed(1)}s`;
    if (n < 60_000) return `${Math.round(n / 1000)}s`;
    if (n < 3_600_000) {
      const m = Math.floor(n / 60_000);
      const s = Math.floor((n % 60_000) / 1000);
      return `${m}m${s.toString().padStart(2, "0")}s`;
    }
    const h = Math.floor(n / 3_600_000);
    const m = Math.floor((n % 3_600_000) / 60_000);
    return `${h}h${m.toString().padStart(2, "0")}m`;
  };
  // Compact rate formatter — number + k/M suffix, no whitespace. The
  // separate `unit` argument lives in the tile label, not the value, so
  // the value column stays wide enough to render two numbers when the
  // caller composes (qps + tps).
  const fmtRate = (n: number | undefined, unit?: string): string => {
    if (n === undefined) return "—";
    const abs = Math.abs(n);
    let body: string;
    if (abs >= 1_000_000) body = `${(n / 1_000_000).toFixed(1)}M`;
    else if (abs >= 10_000) body = `${(n / 1000).toFixed(0)}k`;
    else if (abs >= 1000) body = `${(n / 1000).toFixed(1)}k`;
    else if (abs >= 10) body = n.toFixed(0);
    else body = n.toFixed(2);
    return unit ? `${body}${unit}` : body;
  };
  const fmtPct = (n: number | undefined) => (n === undefined ? "—" : `${Math.round(n)}%`);

  if (overviewPoll.error || poolsPoll.error) {
    const err = overviewPoll.error?.message ?? poolsPoll.error?.message ?? "fetch failed";
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Overview</h1>
        <p className="mt-2 text-sm text-danger">
          Could not read overview/pools: {err}. The pooler may be unreachable, or admin credentials may have been rotated.
        </p>
      </section>
    );
  }

  return (
    <div className="flex flex-col">
      <PageHero
        title="Overview"
        description="Live pooler-wide signals. Click a Golden signals tile for the 1-hour panel with p50/p95/p99."
        actions={
          <Link
            to="/wall"
            className="border border-border-strong px-3 py-1.5 text-xs uppercase tracking-wider text-text-muted transition-colors hover:border-accent hover:text-accent"
            title="Open kiosk view (large KPIs + heatmap)"
          >
            war room ↗
          </Link>
        }
      />
      <div className="mx-auto w-full max-w-[1680px] space-y-6 px-6 py-6">
        <FallbackBanner pools={poolsPoll.data?.pools ?? []} />
        <ProcessBar process={processPoll.data} onOpenThreads={() => openPanelById("threads")} onOpenRss={() => openPanelById("rss")} />
        <Card
          title="Golden signals"
          helpStructured={{
            definition:
              "Four pooler-wide signals: backend query p95, traffic, error rate, and worst-pool saturation.",
            source: "SHOW STATS · SHOW POOLS (per-pool aggregated)",
            formula:
              "p95 = max(query_p95 across pools) · traffic = Δqueries / Δt · errors = Δerrors_total / Δt · saturation = max(active / max_connections)",
            thresholds: {
              healthy: "p95 < 100 ms · errors ≈ 0/s · saturation < 70 %",
              warn: "p95 ≥ 100 ms · errors ≥ 1/s · saturation ≥ 70 %",
              crit: "p95 ≥ 500 ms · errors ≥ 10/s · saturation ≥ 90 %",
            },
            related: ["waiting", "max_active_age_ms", "errors_per_s"],
            docsHref:
              "https://ozontech.github.io/pg_doorman/tutorials/overview.html",
          }}
        >
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            <ChartLink onClick={() => openPanelById("latency")}>
              <Sparkline
                label="Latency P95 ↗"
                valueText={fmtMs(latest?.query_p95_max_ms)}
                series={sigSeries((s) => s.query_p95_max_ms)}
                warn={100}
                crit={500}
                logY
                syncKey="overview"
                events={chartEvents}
                tip="Max query p95 across pools. Warn at 100 ms, crit at 500 ms."
              />
            </ChartLink>
            <ChartLink onClick={() => openPanelById("traffic")}>
              <Sparkline
                label="Traffic ↗"
                valueText={`${fmtRate(latest?.qps)} · ${fmtRate(latest?.tps)}`}
                series={sigSeries((s) => s.qps)}
                syncKey="overview"
                events={chartEvents}
                tip="Queries/s on the left, transactions/s on the right. Sparkline tracks q/s."
              />
            </ChartLink>
            <ChartLink onClick={() => openPanelById("errors")}>
              <Sparkline
                label="Errors / s ↗"
                valueText={fmtRate(latest?.errors_per_s)}
                series={sigSeries((s) => s.errors_per_s)}
                warn={1}
                crit={10}
                syncKey="overview"
                events={chartEvents}
                tip="Aggregate errors/s. Warn at 1/s, crit at 10/s."
              />
            </ChartLink>
            <ChartLink onClick={() => openPanelById("saturation")}>
              <Sparkline
                label="Saturation max ↗"
                valueText={fmtPct(latest?.saturation_max_pct)}
                series={sigSeries((s) => s.saturation_max_pct)}
                warn={70}
                crit={90}
                syncKey="overview"
                events={chartEvents}
                tip="Worst pool saturation. Warn at 70 %, crit at 90 %. Heatmap below names the hot pool."
              />
            </ChartLink>
          </div>
        </Card>
        <Card
          title="Connection breakdown ↗"
          onTitleClick={() => openPanelById("conn_breakdown")}
          helpStructured={{
            definition:
              "Stacked client states — active, idle, waiting — across the whole pooler over the last three minutes.",
            source: "SHOW POOLS · cl_active + cl_idle + cl_waiting",
            formula:
              "active = Σ cl_active · idle = Σ cl_idle · waiting = Σ cl_waiting",
            thresholds: {
              healthy: "waiting ≈ 0",
              warn: "waiting > 0 sustained",
              crit: "waiting > 10 sustained 10 s",
            },
            related: ["max_active_age_ms", "wait_p95_ms"],
            docsHref:
              "https://ozontech.github.io/pg_doorman/tutorials/pool-pressure.html",
          }}
        >
          <AreaChart
            data={connBreakdown}
            labels={["active", "idle", "waiting"]}
            fills={["rgb(45 194 107 / 0.55)", "rgb(138 147 164 / 0.45)", "rgb(245 165 36 / 0.55)"]}
            syncKey="overview"
            events={chartEvents}
          />
        </Card>
        {heatmapRows.length > 0 && (
          <Card
            title="Pool fill heatmap"
            helpStructured={{
              definition:
                "One row per pool. Each cell is the pool's saturation at a 1.5 s sample over the last 90 s.",
              source: "SHOW POOLS · cl_active / max_client_conn per pool",
              formula: "saturation = active / max_connections",
              thresholds: {
                healthy: "cells green < 70 %",
                warn: "amber 70–89 %",
                crit: "red ≥ 90 %",
              },
              related: ["wait_p95_ms", "max_active_age_ms"],
              docsHref:
                "https://ozontech.github.io/pg_doorman/tutorials/pool-pressure.html",
            }}
          >
            <Heatmap rows={heatmapRows} />
          </Card>
        )}
        <Card
          title="Wait queue vs oldest active ↗"
          onTitleClick={() => openPanelById("wait_oldest")}
          helpStructured={{
            definition:
              "Clients currently queued for a backend vs the oldest in-flight query across pools.",
            source: "SHOW POOLS · cl_waiting · maxwait_us / 1000",
            formula:
              "waiting = Σ cl_waiting · oldest = max(maxwait_us) across pools",
            thresholds: {
              healthy: "waiting ≈ 0 · oldest < 30 s",
              warn: "oldest ≥ 30 s",
              crit: "oldest ≥ 5 min — stuck transaction",
            },
            related: ["wait_p95_ms", "active_clients", "backend_p99"],
            docsHref:
              "https://ozontech.github.io/pg_doorman/tutorials/troubleshooting.html",
          }}
        >
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
            events={chartEvents}
          />
        </Card>
        <SqlstateTopCard pools={poolsPoll.data?.pools ?? []} />
        {top5Errors.labels.length > 0 && (
          <Card
            title="Top pools by error rate ↗"
            onTitleClick={() => openPanelById("top_errors")}
            help={{
              what: `The ${top5Errors.labels.length} pools with the highest errors-per-second over the last 30 s.`,
              how: "Each band is one pool. Empty = no errors in the last 30 s. A band sustained above 1 err/s for ten samples in a row is the pool that needs the SQLSTATE drill-down.",
              normal: "Bands hovering at 0 = no errors. Sustained > 1 / s on one band = investigate that pool.",
            }}
          >
            <AreaChart
              data={top5Errors.data}
              labels={top5Errors.labels}
              fills={[
                "rgb(229 72 77 / 0.6)",
                "rgb(245 165 36 / 0.6)",
                "rgb(177 140 245 / 0.6)",
                "rgb(91 140 255 / 0.55)",
                "rgb(45 194 107 / 0.5)",
              ]}
              syncKey="overview"
              events={chartEvents}
            />
          </Card>
        )}
        <Collapsible id="overview-resource" title="Resource detail">
          <ResourceDetail sockets={socketsPoll.data} interner={internerPoll.data} />
        </Collapsible>
      </div>

      {openPanel === "rss" && <MemoryPanel open onClose={closePanel} />}
      {openPanel && openPanel !== "rss" && (
        <PanelView
          {...panelDescriptor(
            openPanel,
            seriesXs,
            sampleHistory.history,
            connBreakdown,
            top5Errors,
            chartEvents,
            threadHistoryRef.current,
            processSnapshotsRef.current,
          )}
          onClose={closePanel}
        />
      )}
    </div>
  );
}

function Card({
  title,
  help,
  helpStructured,
  children,
  onTitleClick,
}: {
  title: string;
  help?: { what?: string; how?: string; normal?: string };
  helpStructured?: import("../components/HelpTip").HelpContent;
  children: ReactNode;
  onTitleClick?: () => void;
}) {
  // When the section is bound to a PanelView (`onTitleClick` set) we make
  // the whole card clickable, not just the title text. Operators expected
  // the chart-title arrow ↗ to be a clue that the card opens, but they
  // kept clicking the canvas instead of the heading. The wrapper button
  // takes the click anywhere; the canvas itself still owns mouse-move for
  // hover readout.
  const inner = (
    <>
      <SectionHeader
        title={title}
        help={helpStructured}
        what={help?.what}
        how={help?.how}
        normal={help?.normal}
        onTitleClick={onTitleClick}
      />
      <div className="p-4">{children}</div>
    </>
  );
  if (onTitleClick) {
    return (
      <section
        role="button"
        tabIndex={0}
        onClick={onTitleClick}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onTitleClick();
          }
        }}
        className="cursor-pointer rounded-md border border-border bg-surface shadow-card transition-all duration-150 hover:-translate-y-px hover:border-border-strong hover:shadow-md"
        title="Open in panel view (1h history, p50/p95/p99 table)."
      >
        {inner}
      </section>
    );
  }
  return <section className="rounded-md border border-border bg-surface shadow-card">{inner}</section>;
}

// Wrapper that turns a Sparkline card into a button-like region: any click
// inside (other than on the cursor itself, which uPlot prevents from
// bubbling) opens the matching PanelView. Keyboard activation via Enter
// keeps the affordance accessible for non-mouse users.
function ChartLink({
  onClick,
  children,
}: {
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick();
        }
      }}
      className="cursor-pointer transition-colors hover:bg-surface-2"
      title="Open in panel view (1h history, p50/p95/p99 table)."
    >
      {children}
    </div>
  );
}

interface PanelDescriptor {
  open: true;
  title: string;
  kind: PanelKind;
  data: [number[], ...number[][]];
  labels: string[];
  fills?: string[];
  rightSeries?: number[];
  rightLogScale?: boolean;
  warn?: number;
  crit?: number;
  units?: string;
  events?: import("../components/Sparkline").ChartEvent[];
}

function panelDescriptor(
  id: string,
  seriesXs: number[],
  history: OverviewSamplePoint[],
  connBreakdown: [number[], ...number[][]],
  top5Errors: { labels: string[]; data: [number[], ...number[][]] },
  events: import("../components/Sparkline").ChartEvent[],
  threadHistory: Array<{
    ts: number;
    pcts: Map<number, number>;
    names: Map<number, string>;
  }>,
  processSnapshots: ProcessDto[],
): PanelDescriptor {
  switch (id) {
    case "latency":
      return {
        open: true,
        title: "Latency P95 (max across pools)",
        kind: "line",
        data: [seriesXs, history.map((s) => s.query_p95_max_ms)] as [number[], ...number[][]],
        labels: ["query p95"],
        fills: ["rgb(255 176 0)"],
        warn: 100,
        crit: 500,
        units: "ms",
        events,
      };
    case "traffic":
      return {
        open: true,
        title: "Traffic — qps and tps",
        kind: "line",
        data: [
          seriesXs,
          history.map((s) => s.qps),
          history.map((s) => s.tps),
        ] as [number[], ...number[][]],
        labels: ["queries / s", "transactions / s"],
        fills: ["rgb(255 176 0)", "rgb(0 212 255)"],
        units: "/s",
        events,
      };
    case "errors":
      return {
        open: true,
        title: "Errors per second",
        kind: "line",
        data: [seriesXs, history.map((s) => s.errors_per_s)] as [number[], ...number[][]],
        labels: ["errors / s"],
        fills: ["rgb(255 77 77)"],
        warn: 1,
        crit: 10,
        units: "/s",
        events,
      };
    case "saturation":
      return {
        open: true,
        title: "Worst pool saturation %",
        kind: "line",
        data: [seriesXs, history.map((s) => s.saturation_max_pct)] as [number[], ...number[][]],
        labels: ["saturation max %"],
        fills: ["rgb(57 211 83)"],
        warn: 70,
        crit: 90,
        units: "%",
        events,
      };
    case "conn_breakdown":
      return {
        open: true,
        title: "Connection breakdown — active / idle / waiting",
        kind: "stackedArea",
        data: connBreakdown,
        labels: ["active", "idle", "waiting"],
        fills: ["rgb(57 211 83)", "rgb(154 148 133)", "rgb(255 176 0)"],
        events,
      };
    case "wait_oldest":
      return {
        open: true,
        title: "Wait queue vs oldest active query",
        kind: "dualAxis",
        data: [
          seriesXs,
          history.map((s) => s.waiting_clients),
          history.map((s) => Math.max(1, s.oldest_active_age_max_ms)),
        ] as [number[], ...number[][]],
        labels: ["waiting clients", "oldest active ms"],
        fills: ["rgb(0 212 255)", "rgb(255 176 0)"],
        rightSeries: [2],
        rightLogScale: true,
        events,
      };
    case "threads": {
      // Per-thread CPU over the rolling window. We keep only threads that
      // ever exceeded 1% in the window (the rest are bookkeeping overhead
      // — jemalloc background workers idling at 0 — and they bury the
      // signal in the legend). Series order: highest peak first so the
      // legend matches the panel summary.
      const xs = threadHistory.map((s) => s.ts / 1000);
      const peakByTid = new Map<number, number>();
      const nameByTid = new Map<number, string>();
      for (const snap of threadHistory) {
        for (const [tid, pct] of snap.pcts.entries()) {
          if (pct > (peakByTid.get(tid) ?? 0)) peakByTid.set(tid, pct);
          if (!nameByTid.has(tid)) nameByTid.set(tid, snap.names.get(tid) ?? `tid${tid}`);
        }
      }
      const tids = [...peakByTid.entries()]
        .filter(([, peak]) => peak >= 1)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 8)
        .map(([tid]) => tid);
      const seriesPalette = [
        "rgb(255 176 0)",
        "rgb(0 212 255)",
        "rgb(57 211 83)",
        "rgb(255 77 77)",
        "rgb(177 140 245)",
        "rgb(91 140 255)",
        "rgb(245 165 36)",
        "rgb(154 148 133)",
      ];
      const series: number[][] = tids.map((tid) =>
        threadHistory.map((snap) => {
          const v = snap.pcts.get(tid);
          return v === undefined ? NaN : v;
        }),
      );
      const labels = tids.map((tid) => `${nameByTid.get(tid) ?? "tid"}#${tid}`);
      return {
        open: true,
        title: "Per-thread CPU% (active threads only, ≥ 1% peak)",
        kind: "line",
        data: [xs, ...series] as [number[], ...number[][]],
        labels,
        fills: seriesPalette.slice(0, tids.length),
        warn: 60,
        crit: 90,
        units: "% of 1 core",
        events,
      };
    }
    case "rss": {
      // RSS over time + cumulative CPU as secondary line. Memory
      // breakdown research is in flight; until that lands we plot what we
      // already have — the operator at least sees the growth curve.
      const xs = processSnapshots.map((s) => s.ts / 1000);
      const rss = processSnapshots.map((s) => s.rss_bytes / (1024 * 1024));
      const vm = processSnapshots.map((s) => s.vm_size_bytes / (1024 * 1024));
      return {
        open: true,
        title: "Process memory — RSS / VM",
        kind: "line",
        data: [xs, rss, vm] as [number[], ...number[][]],
        labels: ["RSS MiB", "VM MiB"],
        fills: ["rgb(255 176 0)", "rgb(154 148 133 / 0.7)"],
        units: "MiB",
        events,
      };
    }
    case "top_errors":
      return {
        open: true,
        title: `Top ${top5Errors.labels.length} pools by error rate`,
        kind: "stackedArea",
        data: top5Errors.data,
        labels: top5Errors.labels,
        fills: [
          "rgb(255 77 77 / 0.7)",
          "rgb(255 176 0 / 0.7)",
          "rgb(177 140 245 / 0.7)",
          "rgb(91 140 255 / 0.65)",
          "rgb(57 211 83 / 0.6)",
        ],
        events,
      };
    default:
      return {
        open: true,
        title: id,
        kind: "line",
        data: [seriesXs] as [number[], ...number[][]],
        labels: [],
        events,
      };
  }
}


// Banner for Patroni-assisted fallback. The DTO has carried fallback_active
// for releases, but only PoolDetail rendered it; the banner names affected
// pools and links to the docs.
// Aggregated SQLSTATE breakdown across all pools. Closes the gap the
// "Errors / s ↗" tile promised: the tile tooltip claimed it opens a
// SQLSTATE breakdown but the panel was just a line chart, so the
// operator had to drill into every pool individually to find the
// dominant error class. This card sums each pool's errors_by_sqlstate
// and lists the top ten with describeSqlstate() context.
function SqlstateTopCard({ pools }: { pools: PoolDto[] }) {
  const totals = useMemo(() => {
    const m = new Map<string, number>();
    for (const p of pools) {
      const codes = p.errors_by_sqlstate;
      if (!codes) continue;
      for (const [code, count] of Object.entries(codes)) {
        m.set(code, (m.get(code) ?? 0) + count);
      }
    }
    return [...m.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 10);
  }, [pools]);
  if (totals.length === 0) return null;
  return (
    <Card
      title="Top SQLSTATE codes"
      helpStructured={{
        definition:
          "Aggregated error-code frequency across every pool since pg_doorman started. Open Pool detail for per-pool SQLSTATE counts.",
        source: "Σ pool.errors_by_sqlstate across SHOW POOLS",
        related: ["pg_doorman::stats", "pg_stat_activity.last_error"],
        docsHref:
          "https://ozontech.github.io/pg_doorman/tutorials/troubleshooting.html",
      }}
    >
      <ul className="space-y-1 px-4 py-3 text-sm tabular">
        {totals.map(([code, count]) => (
          <li
            key={code}
            className="flex items-baseline justify-between gap-3 border-b border-border/40 py-1 last:border-b-0"
          >
            <span className="flex min-w-0 flex-col">
              <span className="font-mono text-text">{code}</span>
              <span className="truncate text-xs text-text-dim">
                {describeSqlstate(code)}
              </span>
            </span>
            <span className="font-mono text-text">{count}</span>
          </li>
        ))}
      </ul>
    </Card>
  );
}

function FallbackBanner({ pools }: { pools: PoolDto[] }) {
  const inFallback = pools.filter((p) => p.fallback_active);
  if (inFallback.length === 0) return null;
  const names = inFallback.map((p) => p.id).join(", ");
  return (
    <div
      role="alert"
      className="rounded-md border border-warning/40 bg-warning/10 px-4 py-3 text-sm text-warning"
    >
      <span className="font-semibold">Patroni fallback active</span> ·{" "}
      {inFallback.length} pool{inFallback.length === 1 ? "" : "s"} routing to
      the fallback backend.{" "}
      <span className="font-mono text-text-muted">{names}</span>.{" "}
      <a
        href="https://ozontech.github.io/pg_doorman/tutorials/patroni-assisted-fallback.html"
        target="_blank"
        rel="noreferrer noopener"
        className="underline hover:no-underline"
      >
        What is fallback?
      </a>
    </div>
  );
}

function ResourceDetail({
  sockets,
  interner,
}: {
  sockets: SocketsDto | null;
  interner: InternerDto | null;
}) {
  const fmtBytes = (n: number) => {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MiB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GiB`;
  };
  const socketStats: { label: string; value: string }[] = sockets
    ? [
        { label: "tcp established", value: sockets.tcp.established.toString() },
        { label: "tcp time-wait", value: sockets.tcp.time_wait.toString() },
        { label: "tcp close-wait", value: sockets.tcp.close_wait.toString() },
        { label: "tcp listen", value: sockets.tcp.listen.toString() },
        { label: "tcp6 established", value: sockets.tcp6.established.toString() },
        { label: "unix connected", value: sockets.unix_stream.connected.toString() },
      ]
    : [];
  const internerStats: { label: string; value: string }[] = interner
    ? [
        { label: "named entries", value: interner.named.entries.toLocaleString() },
        { label: "named bytes", value: fmtBytes(interner.named.bytes) },
        { label: "anonymous entries", value: interner.anonymous.entries.toLocaleString() },
        { label: "anonymous bytes", value: fmtBytes(interner.anonymous.bytes) },
      ]
    : [];
  return (
    <div className="grid gap-4 px-4 py-4 md:grid-cols-2">
      <ResourceCard title="Sockets" empty={!sockets} emptyLabel="linux only — no data on this platform.">
        <div className="grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-3">
          {socketStats.map((s) => (
            <StatCell key={s.label} label={s.label} value={s.value} />
          ))}
        </div>
      </ResourceCard>
      <ResourceCard title="Query interner" empty={!interner} emptyLabel="loading…">
        <div className="grid grid-cols-2 gap-x-4 gap-y-2">
          {internerStats.map((s) => (
            <StatCell key={s.label} label={s.label} value={s.value} />
          ))}
        </div>
      </ResourceCard>
    </div>
  );
}

function ResourceCard({
  title,
  empty,
  emptyLabel,
  children,
}: {
  title: string;
  empty: boolean;
  emptyLabel: string;
  children: ReactNode;
}) {
  return (
    <div className="border border-border bg-surface">
      <div className="border-b border-border px-4 py-2 text-[10px] font-semibold uppercase tracking-wide text-text-muted">
        {title}
      </div>
      <div className="p-4">
        {empty ? <p className="text-sm text-text-dim">{emptyLabel}</p> : children}
      </div>
    </div>
  );
}

function StatCell({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-[10px] uppercase tracking-wider text-text-dim">
        {label}
      </span>
      <span className="font-mono text-base font-semibold tabular text-text">
        {value}
      </span>
    </div>
  );
}

// Process resource bar — RSS, CPU%, FDs, threads, uptime, hostname/pid.
// CPU% is computed from successive snapshots: every poll we record the
// previous (cpu_user_us + cpu_system_us) and the latest, divide by the
// elapsed wall-clock and the core count to get a percentage of one core.
// "100%" means one core saturated; with N cores fully busy you'd see
// N×100%. The bar paints amber when total CPU > 60% of cpu_cores and red
// when > 90%; FDs paint amber > 70% of limit, red > 90%.
function ProcessBar({
  process,
  onOpenThreads,
  onOpenRss,
}: {
  process: ProcessDto | null;
  onOpenThreads?: () => void;
  onOpenRss?: () => void;
}) {
  // Two refs: the previous snapshot we computed against, and the most
  // recent percentage. Re-renders that don't bring a new ts (a sibling
  // poll updated state) reuse the cached delta instead of nulling it
  // out — without that we'd flicker "sampling…" between every real poll.
  // Persist the previous ProcessDto in localStorage so CPU% and per-thread
  // deltas survive a page navigation. Without this, every reopen of
  // Overview started with "sampling…" until two snapshots accumulated
  // again — and the panel never settled while the operator was busy
  // clicking between pages.
  const prevRef = useRef<ProcessDto | null>(loadPrevProcess());
  const cachedPctRef = useRef<{
    cpuPct: number | null;
    threadDeltas: { tid: number; name: string; pct: number }[];
    forTs: number;
  } | null>(null);

  let cpuPct: number | null = null;
  let threadDeltas: { tid: number; name: string; pct: number }[] = [];
  const last = prevRef.current;
  if (process && cachedPctRef.current && cachedPctRef.current.forTs === process.ts) {
    // Same poll snapshot we already computed against — reuse cached values.
    cpuPct = cachedPctRef.current.cpuPct;
    threadDeltas = cachedPctRef.current.threadDeltas;
  } else if (
    process &&
    last &&
    last.ts !== process.ts &&
    // Drop persisted snapshots that are too old to compute a meaningful
    // delta (laptop slept, tab closed for hours). 60 s window matches
    // the sidebar guard.
    process.ts - last.ts < 60_000
  ) {
    const dtSec = (process.ts - last.ts) / 1000;
    if (dtSec > 0 && process.cpu_cores > 0) {
      const usDelta =
        process.cpu_user_us +
        process.cpu_system_us -
        (last.cpu_user_us + last.cpu_system_us);
      cpuPct = (usDelta / 1_000_000 / dtSec) * 100;

      // Per-thread CPU% deltas. Operators care about the hottest tokio
      // worker — a single worker pinned to 100% means the runtime is
      // imbalanced even when the global CPU number looks fine.
      const lastByTid = new Map<number, number>(
        last.threads_breakdown.map((t) => [t.tid, t.cpu_user_us + t.cpu_system_us]),
      );
      threadDeltas = process.threads_breakdown
        .map((t): { tid: number; name: string; pct: number } | null => {
          const prevTotal = lastByTid.get(t.tid);
          const cur = t.cpu_user_us + t.cpu_system_us;
          if (prevTotal === undefined) return null;
          const deltaUs = cur - prevTotal;
          if (deltaUs <= 0) return { tid: t.tid, name: t.name, pct: 0 };
          return { tid: t.tid, name: t.name, pct: (deltaUs / 1_000_000 / dtSec) * 100 };
        })
        .filter((x: { tid: number; name: string; pct: number } | null): x is { tid: number; name: string; pct: number } => x !== null)
        .sort((a: { pct: number }, b: { pct: number }) => b.pct - a.pct);
    }
  }
  // Stash *after* computing the delta — but only when we actually advanced
  // to a new poll snapshot. Skipping stale re-renders keeps the cached
  // values usable on the next paint.
  if (process && (!last || last.ts !== process.ts)) {
    prevRef.current = process;
    cachedPctRef.current = { cpuPct, threadDeltas, forTs: process.ts };
    savePrevProcess(process);
  }

  if (!process) return null;

  const maxThreadPct = threadDeltas[0]?.pct ?? null;
  const minThreadPct =
    threadDeltas.length > 0 ? threadDeltas[threadDeltas.length - 1].pct : null;
  const avgThreadPct =
    threadDeltas.length > 0
      ? threadDeltas.reduce((s, t) => s + t.pct, 0) / threadDeltas.length
      : null;

  const fmtBytes = (n: number) => {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MiB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GiB`;
  };
  const fmtUptime = (s: number): string => {
    if (s < 60) return `${s}s`;
    const m = Math.floor(s / 60);
    if (m < 60) return `${m}m ${s % 60}s`;
    const h = Math.floor(m / 60);
    if (h < 24) return `${h}h ${m % 60}m`;
    const d = Math.floor(h / 24);
    return `${d}d ${h % 24}h`;
  };
  // FD limits land at 2^30+ on modern Linux containers — formatting them
  // as raw integers turned the tile into "66/1073741816" which truncated
  // and read as gibberish. Operators care that the limit is "effectively
  // infinite", not the exact number; if the limit is sane (< 1M) we show
  // the raw figure, otherwise we abbreviate.
  const fmtFdLimit = (n: number): string => {
    if (n <= 0) return "?";
    if (n < 1_000_000) return n.toLocaleString();
    if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(0)}M`;
    return "∞";
  };

  const cpuTone =
    cpuPct === null
      ? "text-text-muted"
      : cpuPct > 90 * process.cpu_cores
        ? "text-danger"
        : cpuPct > 60 * process.cpu_cores
          ? "text-warning"
          : "text-text";
  const fdRatio = process.fd_limit > 0 ? process.fd_open / process.fd_limit : 0;
  const fdTone =
    fdRatio > 0.9 ? "text-danger" : fdRatio > 0.7 ? "text-warning" : "text-text";
  const maxThreadTone =
    maxThreadPct === null
      ? "text-text-muted"
      : maxThreadPct > 90
        ? "text-danger"
        : maxThreadPct > 60
          ? "text-warning"
          : "text-text";

  return (
    <div className="border border-border bg-surface px-4 py-3">
      <div className="mb-2 flex items-baseline justify-between">
        <span className="text-[10px] uppercase tracking-wide text-text-dim">Process</span>
        <span className="font-mono text-xs text-text-dim">
          {process.hostname || "host"} · pid {process.pid}
        </span>
      </div>
      <div className="grid grid-cols-2 gap-3 text-sm md:grid-cols-3 lg:grid-cols-6">
        <ProcStat
          label="cpu (proc)"
          value={cpuPct === null ? "sampling…" : `${cpuPct.toFixed(0)}%`}
          tone={cpuTone}
          hint={`${process.cpu_cores} cores. 100 % means one core fully busy; ${process.cpu_cores * 100} % means every core fully busy. Sustained > ${60 * process.cpu_cores} % is amber, > ${90 * process.cpu_cores} % is red.`}
        />
        <ProcStat
          label="rss ↗"
          value={fmtBytes(process.rss_bytes)}
          tone="text-text"
          hint={`Resident memory: ${fmtBytes(process.rss_bytes)}, VM size ${fmtBytes(process.vm_size_bytes)}. Click for the breakdown across caches, jemalloc, code/libs, stacks, and swap.`}
          onClick={onOpenRss}
        />
        <ProcStatTwoLine
          label={`threads (${process.threads}) ↗`}
          primary={
            maxThreadPct === null
              ? "sampling…"
              : `${maxThreadPct.toFixed(0)}/${(minThreadPct ?? 0).toFixed(0)}/${(avgThreadPct ?? 0).toFixed(0)}`
          }
          secondary={maxThreadPct === null ? "" : "max/min/avg %"}
          tone={maxThreadTone}
          hint={
            threadDeltas.length === 0
              ? "Per-thread CPU appears after a second sample arrives (about 3 s). Linux only — empty on macOS/Windows. Click for the time-series per worker."
              : `${process.threads} OS threads · max-thread ${maxThreadPct?.toFixed(0)}% · avg ${avgThreadPct?.toFixed(1)}% · min ${minThreadPct?.toFixed(1)}% (each is % of one core). Click for the per-thread time-series.\n\n` +
                `Top-${Math.min(5, threadDeltas.length)}:\n` +
                threadDeltas
                  .slice(0, 5)
                  .map((t) => `${t.pct.toFixed(0).padStart(3, " ")}%  ${t.name}#${t.tid}`)
                  .join("\n")
          }
          onClick={onOpenThreads}
        />
        <ProcStat
          label="fds"
          value={
            process.fd_limit > 0
              ? `${process.fd_open}/${fmtFdLimit(process.fd_limit)}`
              : String(process.fd_open)
          }
          tone={fdTone}
          hint={`Open FDs vs soft cap (${process.fd_limit.toLocaleString()}). Amber at 70 % means you are running out before LimitNOFILE bites; red at 90 % means clients will start failing accept().`}
        />
        <ProcStat
          label="uptime"
          value={fmtUptime(process.uptime_seconds)}
          tone="text-text"
          hint={
            process.started_at_ms > 0
              ? `Started ${new Date(process.started_at_ms).toLocaleString()}`
              : "Process start timestamp not yet captured"
          }
        />
      </div>
    </div>
  );
}

function ProcStat({
  label,
  value,
  tone,
  hint,
  onClick,
}: {
  label: string;
  value: string;
  tone: string;
  hint: string;
  onClick?: () => void;
}) {
  const cls = `border border-border bg-surface-2 px-3 py-2 ${onClick ? "cursor-pointer hover:border-border-strong" : ""}`;
  return (
    <div title={hint} className={cls} onClick={onClick} role={onClick ? "button" : undefined}>
      <div className="text-[10px] uppercase tracking-wide text-text-dim">{label}</div>
      <div className={`mt-1 font-mono text-base font-semibold tabular ${tone}`}>{value}</div>
    </div>
  );
}

function ProcStatTwoLine({
  label,
  primary,
  secondary,
  tone,
  hint,
  onClick,
}: {
  label: string;
  primary: string;
  secondary: string;
  tone: string;
  hint: string;
  onClick?: () => void;
}) {
  const cls = `border border-border bg-surface-2 px-3 py-2 ${onClick ? "cursor-pointer hover:border-border-strong" : ""}`;
  return (
    <div title={hint} className={cls} onClick={onClick} role={onClick ? "button" : undefined}>
      <div className="text-[10px] uppercase tracking-wide text-text-dim">{label}</div>
      <div className={`mt-1 font-mono text-base font-semibold tabular ${tone}`}>{primary}</div>
      {secondary && <div className="text-[10px] text-text-dim">{secondary}</div>}
    </div>
  );
}
