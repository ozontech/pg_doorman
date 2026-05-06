// War-room route. The same data as Overview, painted as six oversize
// tiles with no chrome — meant for a sidebar TV, a war-room panel, or a
// fullscreen window during an outage. Auto-refreshes via the existing
// usePoll hook; cells flash amber on threshold breaches so the operator
// sees the change without reading the digit.

import { useMemo } from "react";
import { Link } from "react-router-dom";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { OverviewDto, PoolsDto } from "../types";

const POLL_MS = 1500;

interface TileInput {
  label: string;
  value: string;
  detail?: string;
  tone: "ok" | "warning" | "danger";
}

export default function Wall() {
  const { authHeader } = useAdminAuth();
  const overview = usePoll<OverviewDto>(
    (signal) => apiGet<OverviewDto>("/api/overview", authHeader, signal),
    POLL_MS,
  );
  const pools = usePoll<PoolsDto>(
    (signal) => apiGet<PoolsDto>("/api/pools", authHeader, signal),
    POLL_MS,
  );

  const tiles = useMemo<TileInput[]>(() => {
    if (!overview.data || !pools.data) return [];
    let maxP95 = 0;
    let maxSat = 0;
    let maxOldest = 0;
    let waitingTotal = 0;
    let critical = 0;
    let degraded = 0;
    let paused = 0;
    for (const p of pools.data.pools) {
      if (p.query_p95_ms > maxP95) maxP95 = p.query_p95_ms;
      if (p.max_connections > 0) {
        const s = (p.connections / p.max_connections) * 100;
        if (s > maxSat) maxSat = s;
      }
      if (p.max_active_age_ms > maxOldest) maxOldest = p.max_active_age_ms;
      waitingTotal += p.waiting;
      if (p.paused) paused += 1;
      if (p.errors_total > 0 && p.query_p95_ms > 500) critical += 1;
      else if (p.query_p95_ms > 100 || p.waiting > 0) degraded += 1;
    }

    const fmtMs = (n: number) => {
      if (n < 1000) return `${Math.round(n)}ms`;
      if (n < 60_000) return `${(n / 1000).toFixed(0)}s`;
      const m = Math.floor(n / 60_000);
      const s = Math.floor((n % 60_000) / 1000);
      return `${m}m${s.toString().padStart(2, "0")}`;
    };

    return [
      {
        label: "max p95",
        value: fmtMs(maxP95),
        detail: maxP95 > 500 ? "crit > 500ms" : maxP95 > 100 ? "warn > 100ms" : "healthy",
        tone: maxP95 > 500 ? "danger" : maxP95 > 100 ? "warning" : "ok",
      },
      {
        label: "errors / s",
        value: overview.data.errors_count_total.toLocaleString(),
        detail: "cumulative",
        tone: overview.data.errors_count_total > 0 ? "warning" : "ok",
      },
      {
        label: "max sat %",
        value: `${maxSat.toFixed(0)}%`,
        detail: maxSat > 90 ? "crit ≥ 90%" : maxSat > 70 ? "warn ≥ 70%" : "healthy",
        tone: maxSat > 90 ? "danger" : maxSat > 70 ? "warning" : "ok",
      },
      {
        label: "waiting",
        value: String(waitingTotal),
        detail: waitingTotal > 0 ? `${critical}c · ${degraded}d` : "queue empty",
        tone: waitingTotal >= 10 ? "danger" : waitingTotal > 0 ? "warning" : "ok",
      },
      {
        label: "oldest active",
        value: fmtMs(maxOldest),
        detail: maxOldest > 300_000 ? "crit > 5m" : maxOldest > 30_000 ? "warn > 30s" : "healthy",
        tone: maxOldest > 300_000 ? "danger" : maxOldest > 30_000 ? "warning" : "ok",
      },
      {
        label: "pools",
        value: `${pools.data.pools.length}`,
        detail:
          paused > 0
            ? `${paused} paused · ${critical} crit · ${degraded} deg`
            : `${critical} crit · ${degraded} deg`,
        tone: critical > 0 ? "danger" : degraded > 0 ? "warning" : paused > 0 ? "warning" : "ok",
      },
    ];
  }, [overview.data, pools.data]);

  if (overview.error || pools.error) {
    return (
      <section className="flex h-screen items-center justify-center bg-bg text-danger font-mono">
        <div>
          <p className="text-lg">{overview.error?.message ?? pools.error?.message}</p>
          <Link to="/overview" className="text-xs text-text-muted hover:text-accent">
            back to overview
          </Link>
        </div>
      </section>
    );
  }
  if (!overview.data || !pools.data) {
    return (
      <section className="flex h-screen items-center justify-center bg-bg text-text-dim font-mono">
        Reading overview and pool snapshots…
      </section>
    );
  }

  const anyCritical = tiles.some((t) => t.tone === "danger");
  return (
    <section
      className={`flex min-h-screen flex-col bg-bg p-6 font-mono transition-colors ${
        anyCritical ? "ring-4 ring-danger/40" : ""
      }`}
    >
      <header className="flex items-baseline justify-between border-b border-border pb-4">
        <div>
          <div className="text-xs uppercase tracking-[0.3em] text-text-dim">pg_doorman war room</div>
          <div className="mt-1 text-2xl font-semibold tabular text-text">
            {pools.data.pools.length} pools · last update {fmtAge(overview.lastUpdated)}
          </div>
        </div>
        <div className="flex gap-2 text-xs">
          <Link
            to="/overview"
            className="border border-border-strong px-3 py-1 uppercase tracking-wider text-text-muted hover:text-accent"
          >
            back
          </Link>
        </div>
      </header>
      <div className="grid flex-1 grid-cols-1 gap-4 py-6 sm:grid-cols-2 lg:grid-cols-3">
        {tiles.map((t) => (
          <Tile key={t.label} {...t} />
        ))}
      </div>
    </section>
  );
}

function Tile({ label, value, detail, tone }: TileInput) {
  const toneClass =
    tone === "danger"
      ? "border-danger text-danger bg-danger/5"
      : tone === "warning"
        ? "border-warning text-warning bg-warning/5"
        : "border-border text-text bg-surface-2";
  return (
    <div className={`flex flex-col justify-between border-2 ${toneClass} p-6`}>
      <div className="text-sm uppercase tracking-[0.25em] opacity-80">{label}</div>
      <div className="text-7xl font-bold tabular leading-none">{value}</div>
      {detail && <div className="text-sm uppercase tracking-wider opacity-70">{detail}</div>}
    </div>
  );
}

function fmtAge(ts: number | null): string {
  if (!ts) return "—";
  const ageSec = Math.round((Date.now() - ts) / 1000);
  if (ageSec < 5) return "now";
  if (ageSec < 60) return `${ageSec}s ago`;
  return `${Math.round(ageSec / 60)}m ago`;
}
