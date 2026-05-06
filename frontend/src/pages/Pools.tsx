import { useMemo, useState } from "react";
import { apiGet } from "../api";
import { PageHero } from "../components/PageHero";
import { SectionHeader } from "../components/SectionHeader";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import { evaluatePool } from "../lib/thresholds";
import type { PoolDto, PoolsDto, Severity } from "../types";

const POLL_MS = 1500;

type SortKey = "id" | "saturation" | "waiting" | "query_p95_ms" | "errors_total";
type SortDir = "asc" | "desc";

const SEV_COLOR: Record<Severity, string> = {
  ok: "border-l-transparent",
  degraded: "border-l-warning",
  critical: "border-l-danger",
};

const SEV_DOT: Record<Severity, string> = {
  ok: "text-text-dim",
  degraded: "text-warning",
  critical: "text-danger",
};

interface Filters {
  query: string;
  severity: Severity | "all";
}

export default function Pools() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<PoolsDto>(
    (signal) => apiGet<PoolsDto>("/api/pools", authHeader, signal),
    POLL_MS,
  );

  const [filters, setFilters] = useState<Filters>({ query: "", severity: "all" });
  const [sortKey, setSortKey] = useState<SortKey>("saturation");
  const [sortDir, setSortDir] = useState<SortDir>("desc");

  const evaluated = useMemo(() => {
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

  const sortIndicator = (key: SortKey) => (sortKey === key ? (sortDir === "asc" ? " ▲" : " ▼") : "");

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
        description="Per-pool table backed by /api/pools polled at 1.5 s. Severity column applies the same threshold engine as the overview health pill."
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
            <th className="cursor-pointer px-4 py-2 text-right" onClick={() => onSort("saturation")}>
              Conn / Max{sortIndicator("saturation")}
            </th>
            <th className="cursor-pointer px-4 py-2 text-right" onClick={() => onSort("waiting")}>
              Waiting{sortIndicator("waiting")}
            </th>
            <th className="cursor-pointer px-4 py-2 text-right" onClick={() => onSort("query_p95_ms")}>
              p95 ms{sortIndicator("query_p95_ms")}
            </th>
            <th className="px-4 py-2 text-right">p99 ms</th>
            <th className="cursor-pointer px-4 py-2 text-right" onClick={() => onSort("errors_total")}>
              Errors total{sortIndicator("errors_total")}
            </th>
            <th className="px-4 py-2 text-left">State</th>
          </tr>
        </thead>
        <tbody>
          {filtered.map((row) => (
            <PoolRow key={row.pool.id} row={row} />
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

function PoolRow({
  row,
}: {
  row: { pool: PoolDto; eval: ReturnType<typeof evaluatePool>; saturation: number };
}) {
  const { pool, saturation } = row;
  const sev = row.eval.severity;
  return (
    <tr className={`border-b border-border border-l-2 ${SEV_COLOR[sev]} hover:bg-surface-2`}>
      <td className="px-4 py-2 font-mono">{pool.id}</td>
      <td className="px-4 py-2 text-text-muted">{pool.pool_mode}</td>
      <td className="px-4 py-2 text-right">
        <span className={saturation >= 0.9 ? "text-danger" : saturation >= 0.7 ? "text-warning" : ""}>
          {pool.connections} / {pool.max_connections}
        </span>{" "}
        <span className="text-text-dim text-xs">({(saturation * 100).toFixed(0)}%)</span>
      </td>
      <td className={`px-4 py-2 text-right ${pool.waiting > 0 ? "text-warning" : ""}`}>{pool.waiting}</td>
      <td className={`px-4 py-2 text-right ${pool.query_p95_ms > 500 ? "text-danger" : pool.query_p95_ms > 100 ? "text-warning" : ""}`}>
        {pool.query_p95_ms}
      </td>
      <td className={`px-4 py-2 text-right ${pool.query_p99_ms > 2000 ? "text-danger" : pool.query_p99_ms > 500 ? "text-warning" : ""}`}>
        {pool.query_p99_ms}
      </td>
      <td className="px-4 py-2 text-right">{pool.errors_total}</td>
      <td className="px-4 py-2">
        <span className={`text-xs ${SEV_DOT[sev]}`} title={row.eval.reasons.join(" · ")}>
          ● {sev.toUpperCase()}
        </span>
      </td>
    </tr>
  );
}
