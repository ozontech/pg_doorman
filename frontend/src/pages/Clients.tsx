import { useCallback, useMemo, useState } from "react";
import { apiGet } from "../api";
import { PageHero } from "../components/PageHero";
import { SectionHeader } from "../components/SectionHeader";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { ClientsDto } from "../types";

const POLL_MS = 1500;
const PAGE_SIZE = 50;

type SortKey = "queries_total" | "errors_total" | "age_seconds" | "current_query_age_ms";
type SortDir = "asc" | "desc";

interface Filters {
  pool: string;
  database: string;
  user: string;
  state: string;
  appName: string;
}

const STATE_OPTIONS = ["", "active", "idle", "waiting", "closing"];

function buildQuery(filters: Filters, sort: SortKey, dir: SortDir, offset: number): string {
  const params = new URLSearchParams();
  params.set("limit", String(PAGE_SIZE));
  params.set("offset", String(offset));
  params.set("sort", sort);
  params.set("order", dir);
  if (filters.pool) params.set("pool", filters.pool);
  if (filters.database) params.set("database", filters.database);
  if (filters.user) params.set("user", filters.user);
  if (filters.state) params.set("state", filters.state);
  if (filters.appName) params.set("application_name", filters.appName);
  return params.toString();
}

export default function Clients() {
  const { authHeader } = useAdminAuth();
  const [filters, setFilters] = useState<Filters>({
    pool: "",
    database: "",
    user: "",
    state: "",
    appName: "",
  });
  const [sort, setSort] = useState<SortKey>("queries_total");
  const [dir, setDir] = useState<SortDir>("desc");
  const [offset, setOffset] = useState(0);

  const query = useMemo(() => buildQuery(filters, sort, dir, offset), [filters, sort, dir, offset]);
  const fetcher = useCallback(
    (signal: AbortSignal) => apiGet<ClientsDto>(`/api/clients?${query}`, authHeader, signal),
    [authHeader, query],
  );
  const poll = usePoll<ClientsDto>(fetcher, POLL_MS);

  const onSort = (key: SortKey) => {
    if (key === sort) {
      setDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSort(key);
      setDir("desc");
    }
    setOffset(0);
  };
  const sortIndicator = (key: SortKey) => (sort === key ? (dir === "asc" ? " ▲" : " ▼") : "");
  const updateFilter = (k: keyof Filters, v: string) => {
    setFilters((f) => ({ ...f, [k]: v }));
    setOffset(0);
  };

  const total = poll.data?.total ?? 0;
  const showingFrom = total === 0 ? 0 : offset + 1;
  const showingTo = Math.min(total, offset + (poll.data?.clients.length ?? 0));
  const canPrev = offset > 0;
  const canNext = offset + PAGE_SIZE < total;

  if (poll.error) {
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Clients</h1>
        <p className="mt-2 text-sm text-danger">{poll.error.message}</p>
      </section>
    );
  }

  return (
    <section className="flex flex-col">
      <PageHero
        title="Clients"
        description="Every connected client. Polled at 1.5 s through /api/clients with server-side filtering, sorting, and pagination — none of the search work happens in the browser."
      />
      <SectionHeader
        title="Filters"
        what="Substring filter on pool / database / user / application_name and an exact state match."
        how="Each change resets the pager to offset 0 and re-issues the API call."
        normal="Page size is 50 rows; total count below is what the server reports after applying the filters."
      />
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-6 py-3">
        <input
          placeholder="pool"
          value={filters.pool}
          onChange={(e) => updateFilter("pool", e.target.value)}
          className="w-32 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        />
        <input
          placeholder="database"
          value={filters.database}
          onChange={(e) => updateFilter("database", e.target.value)}
          className="w-32 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        />
        <input
          placeholder="user"
          value={filters.user}
          onChange={(e) => updateFilter("user", e.target.value)}
          className="w-32 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        />
        <input
          placeholder="application_name"
          value={filters.appName}
          onChange={(e) => updateFilter("appName", e.target.value)}
          className="w-44 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        />
        <select
          value={filters.state}
          onChange={(e) => updateFilter("state", e.target.value)}
          className="rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        >
          {STATE_OPTIONS.map((s) => (
            <option key={s} value={s}>
              {s === "" ? "any state" : s}
            </option>
          ))}
        </select>
        <span className="ml-auto text-xs text-text-dim tabular">
          {showingFrom}–{showingTo} of {total}
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-3 py-2 text-left">Client</th>
            <th className="px-3 py-2 text-left">Pool</th>
            <th className="px-3 py-2 text-left">App</th>
            <th className="px-3 py-2 text-left">State</th>
            <th className="cursor-pointer px-3 py-2 text-right" onClick={() => onSort("current_query_age_ms")}>
              Q age ms{sortIndicator("current_query_age_ms")}
            </th>
            <th className="cursor-pointer px-3 py-2 text-right" onClick={() => onSort("age_seconds")}>
              Age s{sortIndicator("age_seconds")}
            </th>
            <th className="cursor-pointer px-3 py-2 text-right" onClick={() => onSort("queries_total")}>
              Queries{sortIndicator("queries_total")}
            </th>
            <th className="cursor-pointer px-3 py-2 text-right" onClick={() => onSort("errors_total")}>
              Errors{sortIndicator("errors_total")}
            </th>
            <th className="px-3 py-2 text-left">TLS</th>
          </tr>
        </thead>
        <tbody>
          {poll.data?.clients.map((c) => (
            <tr key={c.client_id} className="border-b border-border hover:bg-surface-2">
              <td className="px-3 py-1.5 font-mono text-xs">{c.client_id}</td>
              <td className="px-3 py-1.5 text-xs">{c.user}@{c.database}</td>
              <td className="px-3 py-1.5 text-xs text-text-muted">{c.application_name || "—"}</td>
              <td className="px-3 py-1.5 text-xs">
                <span className={
                  c.state === "active" ? "text-success" :
                  c.state === "waiting" ? "text-warning" :
                  "text-text-muted"
                }>
                  {c.state}
                </span>
              </td>
              <td className="px-3 py-1.5 text-right">{c.current_query_age_ms || "—"}</td>
              <td className="px-3 py-1.5 text-right">{c.age_seconds}</td>
              <td className="px-3 py-1.5 text-right">{c.queries_total}</td>
              <td className={`px-3 py-1.5 text-right ${c.errors_total > 0 ? "text-warning" : ""}`}>
                {c.errors_total}
              </td>
              <td className="px-3 py-1.5 text-xs text-text-muted">{c.tls ? "✓" : ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="flex items-center gap-2 px-4 py-3">
        <button
          type="button"
          disabled={!canPrev}
          onClick={() => setOffset((o) => Math.max(0, o - PAGE_SIZE))}
          className="rounded border border-border-strong bg-surface-2 px-3 py-1 text-sm text-text disabled:opacity-40"
        >
          ← prev
        </button>
        <button
          type="button"
          disabled={!canNext}
          onClick={() => setOffset((o) => o + PAGE_SIZE)}
          className="rounded border border-border-strong bg-surface-2 px-3 py-1 text-sm text-text disabled:opacity-40"
        >
          next →
        </button>
        {!poll.data && <span className="text-sm text-text-dim">loading…</span>}
      </div>
    </section>
  );
}
