import { useEffect, useMemo, useState } from "react";
import { apiGet } from "../api";
import { Drawer } from "../components/Drawer";
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

  const [filters, setFilters] = useState<Filters>({ query: "", severity: "all" });
  const [sortKey, setSortKey] = useState<SortKey>("saturation");
  const [sortDir, setSortDir] = useState<SortDir>("desc");
  const [openId, setOpenId] = useState<string | null>(null);

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

  const openRow = openId
    ? evaluated.find((r) => r.pool.id === openId) ?? null
    : null;

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
        description="Per-pool table backed by /api/pools, polled every 1.5 s. Each row inlines mini-sparklines for the four signals operators care about; click a row for the full per-pool drawer."
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
              onOpen={() => setOpenId(row.pool.id)}
            />
          ))}
        </tbody>
      </table>
      {filtered.length === 0 && evaluated.length > 0 && (
        <p className="px-4 py-4 text-sm text-text-dim">No pools match the current filter.</p>
      )}
      {!poll.data && <p className="px-4 py-4 text-sm text-text-dim">Loading…</p>}
      <Drawer
        open={openRow !== null}
        title={openRow?.pool.id ?? ""}
        onClose={() => setOpenId(null)}
      >
        {openRow && (
          <PoolDetail
            row={openRow}
            satSeries={seriesFor(openRow.pool.id, (s) => s.saturation * 100)}
            p95Series={seriesFor(openRow.pool.id, (s) => s.query_p95_ms)}
            waitingSeries={seriesFor(openRow.pool.id, (s) => s.waiting)}
          />
        )}
      </Drawer>
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
      <td className="px-4 py-2 text-right">
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
          <MiniSparkline values={satSeries} stroke={satColor} min={0} max={100} />
          <MiniSparkline
            values={p95Series}
            stroke={pool.query_p95_ms > 500 ? "rgb(229 72 77)" : pool.query_p95_ms > 100 ? "rgb(245 165 36)" : "rgb(34 184 207)"}
          />
        </div>
      </td>
      <td className={`px-4 py-2 text-right ${pool.waiting > 0 ? "text-warning" : ""}`}>
        {pool.waiting}
      </td>
      <td
        className={`px-4 py-2 text-right ${
          pool.query_p95_ms > 500 ? "text-danger" : pool.query_p95_ms > 100 ? "text-warning" : ""
        }`}
      >
        {pool.query_p95_ms}
      </td>
      <td className="px-4 py-2 text-right">{pool.errors_total}</td>
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

function PoolDetail({
  row,
  satSeries,
  p95Series,
  waitingSeries,
}: {
  row: Row;
  satSeries: number[];
  p95Series: number[];
  waitingSeries: number[];
}) {
  const { pool } = row;
  return (
    <div className="space-y-6 text-sm">
      <div>
        <div className="text-xs uppercase tracking-wide text-text-dim">Identity</div>
        <dl className="mt-2 space-y-1 tabular">
          <KV label="user" value={pool.user} />
          <KV label="database" value={pool.database} />
          <KV label="upstream" value={`${pool.host}:${pool.port}`} />
          <KV label="mode" value={pool.pool_mode} />
          <KV label="paused" value={pool.paused ? "yes" : "no"} />
          <KV label="epoch" value={String(pool.epoch)} />
        </dl>
      </div>
      <DetailChart label="Saturation %" values={satSeries} stroke="rgb(34 184 207)" min={0} max={100} />
      <DetailChart label="Query p95 ms" values={p95Series} stroke="rgb(245 165 36)" />
      <DetailChart label="Waiting clients" values={waitingSeries} stroke="rgb(91 140 255)" />
      <div>
        <div className="text-xs uppercase tracking-wide text-text-dim">Latency</div>
        <dl className="mt-2 space-y-1 tabular">
          <KV label="query p95 / p99" value={`${pool.query_p95_ms} / ${pool.query_p99_ms} ms`} />
          <KV
            label="transactions p95 / p99"
            value={`${pool.transactions_p95_ms} / ${pool.transactions_p99_ms} ms`}
          />
          <KV label="wait avg / p95" value={`${pool.wait_avg_ms} / ${pool.wait_p95_ms} ms`} />
          <KV label="oldest active" value={`${pool.max_active_age_ms} ms`} />
        </dl>
      </div>
      <div>
        <div className="text-xs uppercase tracking-wide text-text-dim">Counters</div>
        <dl className="mt-2 space-y-1 tabular">
          <KV label="queries total" value={String(pool.queries_total)} />
          <KV label="transactions total" value={String(pool.transactions_total)} />
          <KV label="errors total" value={String(pool.errors_total)} />
        </dl>
      </div>
      {row.eval.reasons.length > 0 && (
        <div>
          <div className="text-xs uppercase tracking-wide text-text-dim">Threshold reasons</div>
          <ul className="mt-2 space-y-1 text-text-muted">
            {row.eval.reasons.map((r) => (
              <li key={r}>· {r}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function KV({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between">
      <dt className="text-text-muted">{label}</dt>
      <dd className="text-text">{value}</dd>
    </div>
  );
}

function DetailChart({
  label,
  values,
  stroke,
  min,
  max,
}: {
  label: string;
  values: number[];
  stroke: string;
  min?: number;
  max?: number;
}) {
  return (
    <div>
      <div className="mb-1 flex items-baseline justify-between">
        <span className="text-xs uppercase tracking-wide text-text-dim">{label}</span>
        <span className="text-xs text-text-dim tabular">last {values.length} samples</span>
      </div>
      <div className="border border-border bg-surface-2 px-2 py-2">
        <MiniSparkline values={values} stroke={stroke} width={360} height={48} min={min} max={max} />
      </div>
    </div>
  );
}
