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
  addr: string;
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
  if (filters.addr) params.set("addr", filters.addr);
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
    addr: "",
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
        <p className="mt-2 text-sm text-danger">
          Could not load clients: {poll.error.message}. Try Sign out → Sign in to refresh credentials, or check whether pg_doorman is running.
        </p>
      </section>
    );
  }

  return (
    <section className="flex flex-col">
      <PageHero
        title="Clients"
        description="Identify a specific client session. Filter by application_name when one app is misbehaving; by addr when you have an IP from pg_stat_activity; by user/database when an account is the suspect. Sort by Q age ms to find a stuck query; by Age s to find a long-lived session; by Errors to find the noisy ones. State = waiting means the client is queued for a backend connection."
      />
      <SectionHeader
        title="Filters"
        what="Substring filter on pool / database / user / application_name and an exact state match."
        how="Each filter change jumps you back to page 1."
        normal="50 rows per page; the count on the right is the filtered total — change a filter to see how it shrinks."
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
        <input
          placeholder="addr (e.g. 10.0.5. or 1.2.3.4:5432)"
          title="Type any fragment of the client's ip:port. Examples: 10.0.5. for a subnet, :5432 for a port, 1.2.3.4 for a single host."
          value={filters.addr}
          onChange={(e) => updateFilter("addr", e.target.value)}
          className="w-56 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text font-mono"
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
            <th className="px-3 py-2 text-left">Addr</th>
            <th className="px-3 py-2 text-left">Pool</th>
            <th className="px-3 py-2 text-left">App</th>
            <th className="px-3 py-2 text-left">State</th>
            <th className="px-3 py-2 text-right" title="Why the client is waiting (e.g. lock, server) and for how long.">
              Wait
            </th>
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
            <th className="px-3 py-2 text-left" title="Client connected over TLS. Empty cell = plaintext on the loopback or on a trusted network.">TLS</th>
          </tr>
        </thead>
        <tbody>
          {poll.data?.clients.map((c) => (
            <tr key={c.client_id} className="border-b border-border hover:bg-surface-2">
              <td className="px-3 py-1.5 font-mono text-xs">{c.client_id}</td>
              <td className="px-3 py-1.5 font-mono text-xs text-text-muted">{c.addr || "—"}</td>
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
              <td className="px-3 py-1.5 text-right text-xs text-text-muted">
                {c.wait && c.wait !== "none" ? (
                  <span title={`reason: ${c.wait}`}>{c.wait_ms ? `${c.wait_ms} ms` : c.wait}</span>
                ) : (
                  "—"
                )}
              </td>
              <td className="px-3 py-1.5 text-right">{c.current_query_age_ms || "—"}</td>
              <td className="px-3 py-1.5 text-right">{c.age_seconds}</td>
              <td className="px-3 py-1.5 text-right">{c.queries_total}</td>
              <td className={`px-3 py-1.5 text-right ${c.errors_total > 0 ? "text-warning" : ""}`}>
                {c.errors_total}
              </td>
              <td className="px-3 py-1.5 text-xs text-text-muted" title={c.tls ? "TLS" : "plaintext"}>
                {c.tls ? "✓" : ""}
              </td>
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
