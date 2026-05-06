import { useEffect, useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { apiGet } from "../api";
import { MiniSparkline } from "../components/MiniSparkline";
import { PageHero } from "../components/PageHero";
import { SectionHeader } from "../components/SectionHeader";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useHistory } from "../hooks/useHistory";
import { usePoll } from "../hooks/usePoll";
import { evaluatePool, type PoolEvaluation } from "../lib/thresholds";
import type { PoolDto, PoolsDto, Severity } from "../types";

const POLL_MS = 1500;
const HISTORY_KEY = "pools";

type SortKey = "id" | "saturation" | "waiting" | "query_p95_ms" | "errors_total";
type SortDir = "asc" | "desc";

const SEV_COLOR: Record<Severity, string> = {
  ok: "border-l-transparent",
  degraded: "border-l-warning",
  critical: "border-l-danger",
};

const SEV_LABEL: Record<Severity, string> = {
  ok: "ok",
  degraded: "degraded",
  critical: "critical",
};

const SEV_TEXT: Record<Severity, string> = {
  ok: "text-text-dim",
  degraded: "text-warning",
  critical: "text-danger",
};

interface Filters {
  query: string;
  severity: Severity | "all";
}

interface RowSnap {
  ts: number;
  saturation: number;
  query_p95_ms: number;
  errors_total: number;
  waiting: number;
}

interface Row {
  pool: PoolDto;
  eval: PoolEvaluation;
  saturation: number;
}

export default function Pools() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<PoolsDto>(
    (signal) => apiGet<PoolsDto>("/api/pools", authHeader, signal),
    POLL_MS,
  );
  const snapHistory = useHistory<Record<string, RowSnap>>(HISTORY_KEY);

  useEffect(() => {
    if (!poll.data) return;
    const ts = poll.data.ts;
    const snap: Record<string, RowSnap> = {};
    for (const p of poll.data.pools) {
      snap[p.id] = {
        ts,
        saturation: p.max_connections > 0 ? p.connections / p.max_connections : 0,
        query_p95_ms: p.query_p95_ms,
        errors_total: p.errors_total,
        waiting: p.waiting,
      };
    }
    snapHistory.push(snap);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [poll.data?.ts]);

  const navigate = useNavigate();
  // URL-state for filters/sort. Operators can paste "/pools?severity=critical&q=app@db"
  // straight into Slack during an incident — every control on the page is a
  // search-param. Local React state mirrors the URL so the inputs stay
  // responsive while the URL updates on commit.
  const [searchParams, setSearchParams] = useSearchParams();
  const initialFilters: Filters = {
    query: searchParams.get("q") ?? "",
    severity: ((searchParams.get("severity") as Severity | null) ??
      ("all" as const)) as Filters["severity"],
  };
  const initialSortKey = (searchParams.get("sort") as SortKey | null) ?? "saturation";
  const initialSortDir = (searchParams.get("dir") as SortDir | null) ?? "desc";
  const [filters, setFiltersState] = useState<Filters>(initialFilters);
  const [sortKey, setSortKeyState] = useState<SortKey>(initialSortKey);
  const [sortDir, setSortDirState] = useState<SortDir>(initialSortDir);

  const syncUrl = (next: { filters?: Filters; sortKey?: SortKey; sortDir?: SortDir }) => {
    const f = next.filters ?? filters;
    const sk = next.sortKey ?? sortKey;
    const sd = next.sortDir ?? sortDir;
    const sp = new URLSearchParams(searchParams);
    if (f.query) sp.set("q", f.query);
    else sp.delete("q");
    if (f.severity !== "all") sp.set("severity", f.severity);
    else sp.delete("severity");
    if (sk !== "saturation") sp.set("sort", sk);
    else sp.delete("sort");
    if (sd !== "desc") sp.set("dir", sd);
    else sp.delete("dir");
    setSearchParams(sp, { replace: true });
  };
  const setFilters = (
    update: Filters | ((prev: Filters) => Filters),
  ) => {
    setFiltersState((prev) => {
      const value = typeof update === "function" ? update(prev) : update;
      syncUrl({ filters: value });
      return value;
    });
  };
  const setSortKey = (key: SortKey) => {
    setSortKeyState(key);
    syncUrl({ sortKey: key });
  };
  const setSortDir = (
    update: SortDir | ((prev: SortDir) => SortDir),
  ) => {
    setSortDirState((prev) => {
      const value = typeof update === "function" ? update(prev) : update;
      syncUrl({ sortDir: value });
      return value;
    });
  };

  const evaluated: Row[] = useMemo(() => {
    if (!poll.data) return [];
    return poll.data.pools.map((p) => ({
      pool: p,
      eval: evaluatePool(p, undefined),
      saturation: p.max_connections > 0 ? p.connections / p.max_connections : 0,
    }));
  }, [poll.data]);

  const filtered = useMemo(() => {
    return evaluated
      .filter((row) => {
        if (filters.query && !row.pool.id.toLowerCase().includes(filters.query.toLowerCase())) return false;
        if (filters.severity !== "all" && row.eval.severity !== filters.severity) return false;
        return true;
      })
      .sort((a, b) => {
        let av: number | string;
        let bv: number | string;
        switch (sortKey) {
          case "id":
            av = a.pool.id;
            bv = b.pool.id;
            break;
          case "saturation":
            av = a.saturation;
            bv = b.saturation;
            break;
          case "waiting":
            av = a.pool.waiting;
            bv = b.pool.waiting;
            break;
          case "query_p95_ms":
            av = a.pool.query_p95_ms;
            bv = b.pool.query_p95_ms;
            break;
          case "errors_total":
            av = a.pool.errors_total;
            bv = b.pool.errors_total;
            break;
        }
        const cmp = av < bv ? -1 : av > bv ? 1 : 0;
        return sortDir === "asc" ? cmp : -cmp;
      });
  }, [evaluated, filters, sortKey, sortDir]);

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir(key === "id" ? "asc" : "desc");
    }
  };
  const sortIndicator = (key: SortKey) =>
    sortKey === key ? (sortDir === "asc" ? " ▲" : " ▼") : "";

  const seriesFor = (poolId: string, extract: (s: RowSnap) => number): number[] =>
    snapHistory.history.map((snap) => snap[poolId] ?? null).filter((v): v is RowSnap => v !== null).map(extract);

  if (poll.error) {
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Pools</h1>
        <p className="mt-2 text-sm text-danger">{poll.error.message}</p>
      </section>
    );
  }

  return (
    <section className="flex flex-col">
      <PageHero
        title="Pools"
        description="One row per pool. Saturation, query latency p95, waiting clients, error count — the four signals that say whether the pool is healthy. Severity column reads from the threshold engine; click a row for the full pool drill-down."
      />
      <SectionHeader
        title="Filter & sort"
        what="Substring filter on pool id and an exact severity match."
        how="Click any column header to sort; click again to flip the direction."
        normal="Default sort is saturation descending — busiest pool floats up."
      />
      <div className="flex items-center gap-3 border-b border-border px-6 py-3">
        <input
          placeholder="filter by id…"
          value={filters.query}
          onChange={(e) => setFilters((f) => ({ ...f, query: e.target.value }))}
          className="rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        />
        <select
          value={filters.severity}
          onChange={(e) => setFilters((f) => ({ ...f, severity: e.target.value as Filters["severity"] }))}
          className="rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        >
          <option value="all">all severities</option>
          <option value="ok">ok</option>
          <option value="degraded">degraded</option>
          <option value="critical">critical</option>
        </select>
        <span className="ml-auto text-xs text-text-dim tabular">
          {filtered.length} of {evaluated.length} pools
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="cursor-pointer px-4 py-2 text-left" onClick={() => onSort("id")}>
              Pool{sortIndicator("id")}
            </th>
            <th className="px-4 py-2 text-left">Mode</th>
            <th
              className="cursor-pointer px-4 py-2 text-right"
              onClick={() => onSort("saturation")}
            >
              Saturation{sortIndicator("saturation")}
            </th>
            <th className="px-4 py-2 text-center">Trend</th>
            <th
              className="cursor-pointer px-4 py-2 text-right"
              onClick={() => onSort("waiting")}
            >
              Waiting{sortIndicator("waiting")}
            </th>
            <th
              className="cursor-pointer px-4 py-2 text-right"
              onClick={() => onSort("query_p95_ms")}
            >
              p95 ms{sortIndicator("query_p95_ms")}
            </th>
            <th
              className="cursor-pointer px-4 py-2 text-right"
              onClick={() => onSort("errors_total")}
            >
              Errors{sortIndicator("errors_total")}
            </th>
            <th className="px-4 py-2 text-left">State</th>
          </tr>
        </thead>
        <tbody>
          {filtered.map((row) => (
            <PoolRowView
              key={row.pool.id}
              row={row}
              satSeries={seriesFor(row.pool.id, (s) => s.saturation * 100)}
              p95Series={seriesFor(row.pool.id, (s) => s.query_p95_ms)}
              onOpen={() => navigate(`/pools/${encodeURIComponent(row.pool.id)}`)}
            />
          ))}
        </tbody>
      </table>
      {filtered.length === 0 && evaluated.length > 0 && (
        <p className="px-4 py-4 text-sm text-text-dim">No pools match the current filter.</p>
      )}
      {!poll.data && <p className="px-4 py-4 text-sm text-text-dim">Loading…</p>}
    </section>
  );
}

function PoolRowView({
  row,
  satSeries,
  p95Series,
  onOpen,
}: {
  row: Row;
  satSeries: number[];
  p95Series: number[];
  onOpen: () => void;
}) {
  const { pool, saturation } = row;
  const sev = row.eval.severity;
  const satColor =
    saturation >= 0.9 ? "rgb(229 72 77)" : saturation >= 0.7 ? "rgb(245 165 36)" : "rgb(45 194 107)";
  return (
    <tr
      className={`cursor-pointer border-b border-border border-l-2 ${SEV_COLOR[sev]} hover:bg-surface-2`}
      onClick={onOpen}
    >
      <td className="px-4 py-2 font-mono">{pool.id}</td>
      <td className="px-4 py-2 text-text-muted">{pool.pool_mode}</td>
      <td
        className="px-4 py-2 text-right"
        title={`Saturation = connections / max_connections. Warn ≥ 70%, crit ≥ 90%. Now: ${pool.connections}/${pool.max_connections} = ${(saturation * 100).toFixed(0)}%`}
      >
        <span
          className={
            saturation >= 0.9 ? "text-danger" : saturation >= 0.7 ? "text-warning" : ""
          }
        >
          {pool.connections}/{pool.max_connections}
        </span>{" "}
        <span className="text-text-dim text-xs">
          ({(saturation * 100).toFixed(0)}%)
        </span>
      </td>
      <td className="px-4 py-2">
        <div className="flex items-center justify-center gap-2">
          <span title={`Saturation last 60 s (current ${(saturation * 100).toFixed(0)}%)`}>
            <MiniSparkline values={satSeries} stroke={satColor} min={0} max={100} />
          </span>
          <span title={`Query p95 last 60 s (current ${pool.query_p95_ms} ms; warn > 100, crit > 500)`}>
            <MiniSparkline
              values={p95Series}
              stroke={pool.query_p95_ms > 500 ? "rgb(229 72 77)" : pool.query_p95_ms > 100 ? "rgb(245 165 36)" : "rgb(34 184 207)"}
            />
          </span>
        </div>
      </td>
      <td
        className={`px-4 py-2 text-right ${pool.waiting > 0 ? "text-warning" : ""}`}
        title={`Waiting clients (queue depth). Sustained ≥ 1 for 10 s = degraded; ≥ max(10, 0.10×max_connections) = critical. Now: ${pool.waiting}`}
      >
        {pool.waiting}
      </td>
      <td
        className={`px-4 py-2 text-right ${
          pool.query_p95_ms > 500 ? "text-danger" : pool.query_p95_ms > 100 ? "text-warning" : ""
        }`}
        title={`Query p95 over last 60 s. Warn > 100 ms, crit > 500 ms. p99 = ${pool.query_p99_ms} ms`}
      >
        {pool.query_p95_ms}
      </td>
      <td
        className="px-4 py-2 text-right"
        title={`Cumulative errors since pool warm-up. Click row for SQLSTATE breakdown.`}
      >
        {pool.errors_total}
      </td>
      <td className="px-4 py-2">
        <span
          className={`text-xs ${SEV_TEXT[sev]}`}
          title={row.eval.reasons.join(" · ")}
        >
          ● {SEV_LABEL[sev].toUpperCase()}
        </span>
      </td>
    </tr>
  );
}

