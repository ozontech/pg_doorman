import { useEffect, useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { InfoLabel } from "../components/InfoLabel";
import { tip } from "../lib/tooltips";
import { apiGet } from "../api";
import { MiniSparkline } from "../components/MiniSparkline";
import { PageHero } from "../components/PageHero";
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
  ok: "text-success",
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
        saturation: p.max_connections > 0 ? p.active / p.max_connections : 0,
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

  // Single source of truth for URL ↔ state sync. Callers always pass the
  // full target state for the dimensions they want to change; the helper
  // re-derives the URL from those values plus the still-current ones for
  // unchanged dimensions. Without this shape, two consecutive
  // setSearchParams calls in one handler (e.g. flipping sortKey then
  // sortDir) read stale closures and the URL ends up sorted by one key
  // while the UI is sorted by another.
  const writeUrl = (f: Filters, sk: SortKey, sd: SortDir) => {
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
      writeUrl(value, sortKey, sortDir);
      return value;
    });
  };

  const evaluated: Row[] = useMemo(() => {
    if (!poll.data) return [];
    return poll.data.pools.map((p) => ({
      pool: p,
      eval: evaluatePool(p, undefined),
      saturation: p.max_connections > 0 ? p.active / p.max_connections : 0,
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
    const nextDir: SortDir =
      key === sortKey ? (sortDir === "asc" ? "desc" : "asc") : key === "id" ? "asc" : "desc";
    setSortKeyState(key);
    setSortDirState(nextDir);
    writeUrl(filters, key, nextDir);
  };
  const sortIndicator = (key: SortKey) =>
    sortKey === key ? (sortDir === "asc" ? " ▲" : " ▼") : "";

  const seriesFor = (poolId: string, extract: (s: RowSnap) => number): number[] =>
    snapHistory.history.map((snap) => snap[poolId] ?? null).filter((v): v is RowSnap => v !== null).map(extract);

  if (poll.error) {
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Pools</h1>
        <p className="mt-2 text-sm text-danger">
          Could not load pools: {poll.error.message}. Try Sign out → Sign in to refresh credentials, or check whether pg_doorman is running.
        </p>
      </section>
    );
  }

  return (
    <section className="flex flex-col">
      <PageHero
        title="Pools"
        description="Find the pool that hurts. Sort by Saturation, p95 ms, or Errors — default order is most saturated first. State = degraded when one threshold trips, critical when two stack. Filter substring + severity are URL-persisted, paste a link to share. Click a row for SQLSTATE breakdown and pause/reconnect controls."
      />
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-6 py-3">
        <input
          placeholder="filter by id…"
          value={filters.query}
          onChange={(e) => setFilters((f) => ({ ...f, query: e.target.value }))}
          className="rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        />
        <div className="flex items-center gap-1" role="radiogroup" aria-label="filter by severity">
          {(["all", "ok", "degraded", "critical"] as const).map((s) => {
            const active = filters.severity === s;
            const activeTone =
              s === "critical"
                ? "border-danger bg-danger/15 text-danger"
                : s === "degraded"
                  ? "border-warning bg-warning/15 text-warning"
                  : s === "ok"
                    ? "border-success bg-success/15 text-success"
                    : "border-accent bg-accent/15 text-accent";
            return (
              <button
                key={s}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() =>
                  setFilters((f) => ({ ...f, severity: s as Filters["severity"] }))
                }
                className={`border px-2.5 py-1 text-xs font-medium transition-colors ${
                  active
                    ? activeTone
                    : "border-border-strong bg-surface-2 text-text-muted hover:text-text"
                }`}
              >
                {s}
              </button>
            );
          })}
        </div>
        <span className="ml-auto text-xs text-text-dim tabular">
          {filtered.length} of {evaluated.length} pools
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-4 py-2 text-left">
              <span className="cursor-pointer" onClick={() => onSort("id")}>
                Pool{sortIndicator("id")}
              </span>
            </th>
            <th className="px-4 py-2 text-left">Mode</th>
            <th className="px-4 py-2 text-right">
              <InfoLabel tip={tip.saturation}>
                <span className="cursor-pointer" onClick={() => onSort("saturation")}>
                  Saturation{sortIndicator("saturation")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-4 py-2 text-center">
              <InfoLabel tip="Mini-sparklines: saturation last 60 s (left) and query p95 last 60 s (right).">
                Trend
              </InfoLabel>
            </th>
            <th className="px-4 py-2 text-right">
              <InfoLabel tip={tip.waiting}>
                <span className="cursor-pointer" onClick={() => onSort("waiting")}>
                  Waiting{sortIndicator("waiting")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-4 py-2 text-right">
              <InfoLabel tip={tip.queryP95}>
                <span className="cursor-pointer" onClick={() => onSort("query_p95_ms")}>
                  p95 ms{sortIndicator("query_p95_ms")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-4 py-2 text-right">
              <InfoLabel tip={tip.errorsTotal}>
                <span className="cursor-pointer" onClick={() => onSort("errors_total")}>
                  Errors{sortIndicator("errors_total")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-4 py-2 text-left">
              <InfoLabel tip="Threshold engine verdict: ok / degraded / critical based on saturation, p95, waiting, and errors per second.">
                State
              </InfoLabel>
            </th>
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
        <p className="px-4 py-4 text-sm text-text-dim">Nothing matches the current filter. Clear the search box or pick &lsquo;all severities&rsquo; to widen.</p>
      )}
      {!poll.data && <p className="px-4 py-4 text-sm text-text-dim">Reading pool list…</p>}
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
        title={`active=${pool.active} of max_connections=${pool.max_connections} (${(saturation * 100).toFixed(0)} %). Plus ${pool.connections - pool.active} warm-idle backends still held from prior bursts. Color reflects active / max — above 70 % is amber, above 90 % is red.`}
      >
        <span
          className={
            saturation >= 0.9 ? "text-danger" : saturation >= 0.7 ? "text-warning" : ""
          }
        >
          {pool.active}/{pool.max_connections}
        </span>{" "}
        <span className="text-text-dim text-xs">
          ({(saturation * 100).toFixed(0)}%)
        </span>
      </td>
      <td className="px-4 py-2">
        <div className="flex items-center justify-center gap-2">
          <span title={`Saturation last 60 s — now ${(saturation * 100).toFixed(0)} %.`}>
            <MiniSparkline values={satSeries} stroke={satColor} min={0} max={100} />
          </span>
          <span title={`Query p95 last 60 s — now ${pool.query_p95_ms} ms. Sustained > 100 ms = degraded backend; > 500 ms = something is stuck.`}>
            <MiniSparkline
              values={p95Series}
              stroke={pool.query_p95_ms > 500 ? "rgb(229 72 77)" : pool.query_p95_ms > 100 ? "rgb(245 165 36)" : "rgb(34 184 207)"}
            />
          </span>
        </div>
      </td>
      <td
        className={`px-4 py-2 text-right ${pool.waiting > 0 ? "text-warning" : ""}`}
        title={`${pool.waiting} client(s) queued for a backend. Anything sustained above zero for 10 s is degraded. Above ${Math.max(10, Math.round(pool.max_connections * 0.1))} for 10 s is critical for this pool.`}
      >
        {pool.waiting}
      </td>
      <td
        className={`px-4 py-2 text-right ${
          pool.query_p95_ms > 500 ? "text-danger" : pool.query_p95_ms > 100 ? "text-warning" : ""
        }`}
        title={`p95 = ${pool.query_p95_ms} ms, p99 = ${pool.query_p99_ms} ms over last 60 s. > 100 ms is amber, > 500 ms is red.`}
      >
        {pool.query_p95_ms}
      </td>
      <td
        className="px-4 py-2 text-right"
        title={`Total errors since pg_doorman started. Click the row for the SQLSTATE breakdown — the codes are what you need for triage.`}
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

