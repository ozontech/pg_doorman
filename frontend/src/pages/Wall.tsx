// War-room route. A wall-mounted view of the pooler designed for a TV in
// the operations area: chrome (sidebar, page hero, helper popovers) is
// gone, the heatmap takes the top half so a saturated row stands out at
// 5 m across the room, six KPI tiles run beneath it with a 90 s sparkline
// under each number, and recent admin events sit at the bottom so a spike
// can be correlated to the operator action that triggered it. The page
// pulses a red border when any critical signal trips.

import { useEffect, useMemo } from "react";
import { Link, useNavigate } from "react-router-dom";
import { apiGet } from "../api";
import { MiniSparkline } from "../components/MiniSparkline";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useHistory } from "../hooks/useHistory";
import { usePoll } from "../hooks/usePoll";
import type { EventsDto, OverviewDto, PoolsDto } from "../types";

const POLL_MS = 1500;
const HISTORY_KEY = "wall";
const HEATMAP_CELLS = 60;
const SPARKLINE_POINTS = 90;

interface WallSample {
  ts: number;
  query_p95_max_ms: number;
  qps: number;
  errors_per_s: number;
  saturation_max_pct: number;
  waiting_total: number;
  oldest_active_max_ms: number;
}

interface RawTotals {
  ts: number;
  queries: number;
  errors: number;
}

interface PoolSatSnap {
  [poolId: string]: { saturation: number; capacity: number };
}

export default function Wall() {
  const { authHeader } = useAdminAuth();
  const navigate = useNavigate();

  // ESC out of kiosk: a wall display has no sidebar, so without a hotkey
  // an operator has to find the "back to console" affordance behind any
  // OS notifications drifting over the header. Esc is muscle-memory for
  // "leave full-screen view" across Grafana, k9s, htop.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") navigate("/overview");
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [navigate]);
  const overviewPoll = usePoll<OverviewDto>(
    (signal) => apiGet<OverviewDto>("/api/overview", authHeader, signal),
    POLL_MS,
  );
  const poolsPoll = usePoll<PoolsDto>(
    (signal) => apiGet<PoolsDto>("/api/pools", authHeader, signal),
    POLL_MS,
  );
  // Admin-only — silently drops to null for anonymous viewers. The
  // events feed simply does not render in that case; the KPI strip and
  // heatmap above are the load-bearing surfaces.
  const eventsPoll = usePoll<EventsDto>(
    (signal) => apiGet<EventsDto>("/api/events", authHeader, signal),
    3000,
  );

  const rawHistory = useHistory<RawTotals>(`${HISTORY_KEY}.raw`, 200);
  const sampleHistory = useHistory<WallSample>(HISTORY_KEY, 200);
  const satHistory = useHistory<PoolSatSnap>(`${HISTORY_KEY}.sat`, HEATMAP_CELLS);

  useEffect(() => {
    if (!overviewPoll.data || !poolsPoll.data) return;
    const ov = overviewPoll.data;
    const pools = poolsPoll.data;
    const prevRaw = rawHistory.history[rawHistory.history.length - 1];
    // Stale-tab guard: a gap > 5 polling intervals means the browser
    // throttled the timer or the laptop slept; drop the buffer instead
    // of pretending the metric was flat through the pause.
    if (prevRaw && ov.ts - prevRaw.ts > 90_000) {
      rawHistory.replace([]);
      sampleHistory.replace([]);
      satHistory.replace([]);
      return;
    }
    rawHistory.push({
      ts: ov.ts,
      queries: ov.query_count_total,
      errors: ov.errors_count_total,
    });
    let qps = 0;
    let eps = 0;
    if (prevRaw) {
      const dt = (ov.ts - prevRaw.ts) / 1000;
      if (dt > 0) {
        qps = Math.max(0, (ov.query_count_total - prevRaw.queries) / dt);
        eps = Math.max(0, (ov.errors_count_total - prevRaw.errors) / dt);
      }
    }
    let maxP95 = 0;
    let maxSat = 0;
    let maxOldest = 0;
    let waitingTotal = 0;
    const satSnap: PoolSatSnap = {};
    for (const p of pools.pools) {
      if (p.query_p95_ms > maxP95) maxP95 = p.query_p95_ms;
      if (p.max_active_age_ms > maxOldest) maxOldest = p.max_active_age_ms;
      waitingTotal += p.waiting;
      const sat = p.max_connections > 0 ? p.active / p.max_connections : 0;
      if (sat > maxSat) maxSat = sat;
      satSnap[p.id] = { saturation: sat, capacity: p.max_connections };
    }
    sampleHistory.push({
      ts: ov.ts,
      query_p95_max_ms: maxP95,
      qps,
      errors_per_s: eps,
      saturation_max_pct: maxSat * 100,
      waiting_total: waitingTotal,
      oldest_active_max_ms: maxOldest,
    });
    satHistory.push(satSnap);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overviewPoll.data?.ts]);

  const latest = sampleHistory.history[sampleHistory.history.length - 1];
  const recentSamples = sampleHistory.history.slice(-SPARKLINE_POINTS);

  const anyCritical = useMemo(() => {
    if (!latest) return false;
    return (
      latest.saturation_max_pct >= 90 ||
      latest.errors_per_s > 10 ||
      latest.waiting_total >= 10
    );
  }, [latest]);

  const heatmapRows = useMemo(() => {
    if (!poolsPoll.data) return [];
    const ids = poolsPoll.data.pools.map((p) => p.id);
    const capacities = new Map<string, number>();
    for (const p of poolsPoll.data.pools) capacities.set(p.id, p.max_connections);
    const recent = satHistory.history.slice(-HEATMAP_CELLS);
    return ids.map((id) => {
      const cells: (number | null)[] = new Array(HEATMAP_CELLS).fill(null);
      const offset = HEATMAP_CELLS - recent.length;
      for (let i = 0; i < recent.length; i++) {
        const cell = recent[i][id];
        cells[offset + i] = cell ? cell.saturation : null;
      }
      return { label: id, cells, capacity: capacities.get(id) ?? 0 };
    });
  }, [poolsPoll.data, satHistory.history]);

  const events = eventsPoll.data?.events ?? [];
  const lastUpdate = overviewPoll.lastUpdated;
  const collecting = sampleHistory.history.length < 2;

  if (overviewPoll.error || poolsPoll.error) {
    const err =
      overviewPoll.error?.message ?? poolsPoll.error?.message ?? "fetch failed";
    return (
      <section className="flex min-h-screen items-center justify-center bg-bg p-6 text-danger">
        <div className="text-center">
          <p className="text-lg">{err}</p>
          <Link
            to="/overview"
            className="mt-4 inline-block border border-border-strong px-3 py-1 text-xs uppercase tracking-wider text-text-muted hover:text-accent"
          >
            back to overview
          </Link>
        </div>
      </section>
    );
  }

  return (
    <section className="relative flex min-h-screen flex-col bg-bg">
      {anyCritical && (
        // Fixed overlay so the pulse covers the full page regardless of
        // section padding. Pointer-events-none keeps clicks reaching the
        // controls beneath it.
        <div
          aria-hidden="true"
          className="pointer-events-none fixed inset-0 z-50 animate-pulse border-4 border-danger"
        />
      )}

      <header className="flex items-center justify-between gap-4 border-b border-border bg-bg px-6 py-3">
        <div className="flex items-center gap-3">
          <span className="font-mono text-sm font-medium leading-none text-text">
            pg_doorman
          </span>
          <span className="text-[10px] uppercase tracking-wider text-text-dim">
            war room
          </span>
          <span className="ml-2 font-mono text-base font-semibold tabular text-text">
            {poolsPoll.data?.pools.length ?? "—"} pools · updated{" "}
            {fmtAge(lastUpdate)}
          </span>
        </div>
        <Link
          to="/overview"
          title="Back to console (Esc)"
          className="inline-flex items-center gap-2 border-2 border-accent bg-accent px-4 py-2 text-sm font-bold uppercase tracking-wide text-accent-fg transition-colors hover:bg-accent-hover"
        >
          <span aria-hidden="true">←</span> back · esc
        </Link>
      </header>

      <div className="flex-1 space-y-4 p-4">
        <BigHeatmap rows={heatmapRows} collecting={collecting} />

        <KpiGrid
          latest={latest}
          recentSamples={recentSamples}
          poolsCount={poolsPoll.data?.pools.length ?? 0}
          collecting={collecting}
        />

        {events.length > 0 && (
          <div className="border border-border bg-surface px-4 py-3">
            <div className="mb-2 text-[10px] uppercase tracking-wider text-text-dim">
              Recent admin events
            </div>
            <ul className="grid gap-1 font-mono text-xs sm:grid-cols-2 lg:grid-cols-3">
              {events.slice(0, 9).map((e) => (
                <li key={`${e.seq}`} className="text-text-muted">
                  <span className="text-text-dim">{fmtClock(e.ts_ms)}</span>{" "}
                  <span className="font-semibold text-text">{e.target}</span>{" "}
                  <span className="truncate">{e.message}</span>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </section>
  );
}

const HEATMAP_CELL_WIDTH = 16;
const HEATMAP_CELL_HEIGHT = 22;

function heatmapColor(sat: number): string {
  if (sat >= 0.9) return "rgb(229 72 77)";
  if (sat >= 0.7) return "rgb(245 165 36)";
  return "rgb(45 194 107)";
}

function BigHeatmap({
  rows,
  collecting,
}: {
  rows: { label: string; cells: (number | null)[]; capacity: number }[];
  collecting: boolean;
}) {
  return (
    <div className="border border-border bg-surface px-4 py-4">
      <div className="mb-3 flex items-center justify-between text-[11px] uppercase tracking-wider text-text-dim">
        <span>Pool saturation · last 90 s</span>
        <span className="flex items-center gap-3 normal-case tracking-normal text-[10px]">
          <span className="inline-flex items-center gap-1">
            <span
              className="inline-block h-2 w-2"
              style={{ background: "rgb(45 194 107)" }}
            />
            &lt; 70%
          </span>
          <span className="inline-flex items-center gap-1">
            <span
              className="inline-block h-2 w-2"
              style={{ background: "rgb(245 165 36)" }}
            />
            70–89%
          </span>
          <span className="inline-flex items-center gap-1">
            <span
              className="inline-block h-2 w-2"
              style={{ background: "rgb(229 72 77)" }}
            />
            ≥ 90%
          </span>
        </span>
      </div>
      {rows.length === 0 ? (
        <p className="py-6 text-center text-sm text-text-dim">
          {collecting ? "Collecting pool list…" : "No pools."}
        </p>
      ) : (
        <div className="space-y-1.5">
          {rows.map((row) => (
            <div key={row.label} className="flex items-center gap-3">
              <span
                className="min-w-0 flex-1 truncate font-mono text-sm text-text"
                title={row.label}
              >
                {row.label}
              </span>
              <span className="w-14 shrink-0 text-right text-[11px] text-text-dim tabular">
                {row.capacity} max
              </span>
              <div className="flex shrink-0 gap-px">
                {row.cells.map((cell, i) => (
                  <div
                    key={i}
                    style={{
                      width: HEATMAP_CELL_WIDTH,
                      height: HEATMAP_CELL_HEIGHT,
                      background:
                        cell === null ? "rgb(35 42 54)" : heatmapColor(cell),
                      opacity: cell === null ? 0.3 : 1,
                    }}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function KpiGrid({
  latest,
  recentSamples,
  poolsCount,
  collecting,
}: {
  latest: WallSample | undefined;
  recentSamples: WallSample[];
  poolsCount: number;
  collecting: boolean;
}) {
  const seriesOf = (extract: (s: WallSample) => number) =>
    recentSamples.map(extract);

  return (
    <div className="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-6">
      <KpiTile
        label="max p95"
        value={fmtMs(latest?.query_p95_max_ms)}
        tone={tone(latest?.query_p95_max_ms, 100, 500)}
        spark={seriesOf((s) => s.query_p95_max_ms)}
        sparkColor="rgb(255 176 0)"
        collecting={collecting}
      />
      <KpiTile
        label="errors / s"
        value={fmtRate(latest?.errors_per_s)}
        tone={tone(latest?.errors_per_s, 1, 10)}
        spark={seriesOf((s) => s.errors_per_s)}
        sparkColor="rgb(229 72 77)"
        collecting={collecting}
      />
      <KpiTile
        label="max sat"
        value={
          latest === undefined ? "—" : `${latest.saturation_max_pct.toFixed(0)}%`
        }
        tone={tone(latest?.saturation_max_pct, 70, 90)}
        spark={seriesOf((s) => s.saturation_max_pct)}
        sparkColor="rgb(57 211 83)"
        collecting={collecting}
      />
      <KpiTile
        label="waiting"
        value={latest === undefined ? "—" : String(latest.waiting_total)}
        tone={tone(latest?.waiting_total, 1, 10)}
        spark={seriesOf((s) => s.waiting_total)}
        sparkColor="rgb(0 212 255)"
        collecting={collecting}
      />
      <KpiTile
        label="oldest active"
        value={fmtMs(latest?.oldest_active_max_ms)}
        tone={tone(latest?.oldest_active_max_ms, 30_000, 300_000)}
        spark={seriesOf((s) => s.oldest_active_max_ms)}
        sparkColor="rgb(255 176 0)"
        collecting={collecting}
      />
      <KpiTile
        label="pools"
        value={poolsCount > 0 ? `${poolsCount}` : "—"}
        tone="text-text"
        spark={[]}
        sparkColor="rgb(154 148 133)"
        collecting={false}
      />
    </div>
  );
}

function KpiTile({
  label,
  value,
  tone,
  spark,
  sparkColor,
  collecting,
}: {
  label: string;
  value: string;
  tone: string;
  spark: number[];
  sparkColor: string;
  collecting: boolean;
}) {
  const isAlert = tone === "text-danger" || tone === "text-warning";
  return (
    <div
      className={`flex flex-col gap-2 border border-border bg-surface px-4 py-3 border-l-4 ${isAlert ? "border-l-danger" : "border-l-accent"}`}
    >
      <div className="text-[10px] font-semibold uppercase tracking-wider text-text-muted">
        {label}
      </div>
      <div className={`font-mono text-4xl font-extrabold leading-none tabular ${tone}`}>
        {value}
      </div>
      <div className="h-6">
        {spark.length >= 2 ? (
          <MiniSparkline values={spark} stroke={sparkColor} width={200} height={24} />
        ) : collecting ? (
          <span className="text-[10px] text-text-dim">collecting…</span>
        ) : null}
      </div>
    </div>
  );
}

function tone(
  value: number | undefined,
  warn: number,
  crit: number,
): string {
  if (value === undefined) return "text-text-dim";
  if (value >= crit) return "text-danger";
  if (value >= warn) return "text-warning";
  return "text-text";
}

function fmtMs(n: number | undefined): string {
  if (n === undefined) return "—";
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
}

function fmtRate(n: number | undefined): string {
  if (n === undefined) return "—";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${(n / 1000).toFixed(0)}k`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  if (n >= 10) return n.toFixed(0);
  return n.toFixed(2);
}

function fmtAge(ts: number | null): string {
  if (!ts) return "—";
  const ageSec = Math.round((Date.now() - ts) / 1000);
  if (ageSec < 5) return "now";
  if (ageSec < 60) return `${ageSec}s ago`;
  return `${Math.round(ageSec / 60)}m ago`;
}

function fmtClock(tsMs: number): string {
  const d = new Date(tsMs);
  return `${d.getHours().toString().padStart(2, "0")}:${d
    .getMinutes()
    .toString()
    .padStart(2, "0")}:${d.getSeconds().toString().padStart(2, "0")}`;
}
