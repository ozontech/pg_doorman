import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { apiGet } from "../api";
import { InfoLabel } from "../components/InfoLabel";
import { PageHero } from "../components/PageHero";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { ClientDto, ClientsDto } from "../types";

// Slower cadence than the dashboards: a per-client view at 1.5 s caused
// Chrome to balloon in memory — 50 rows × 13 cells re-rendered every poll,
// plus a full ClientRates record allocated each time. 3 s is still
// "live" enough for the operator scanning sessions during an incident,
// without making the GC chase 650 DOM updates per second.
const POLL_MS = 3000;
const PAGE_SIZE = 50;

type ClientTotals = Record<string, { queries: number; transactions: number }>;
type ClientRates = Record<string, { qps: number; tps: number }>;

// Computes per-client qps / tps from the delta between the current /api/clients
// snapshot and the previous one. Mirrors `useAppRates` on the Apps page;
// /api/clients only ships lifetime counters, so without this hook the operator
// has no way to see which client session is busy *right now* short of
// watching the queries column tick.
function useClientRates(data: ClientsDto | null): ClientRates {
  const [rates, setRates] = useState<ClientRates>({});
  const prevRef = useRef<{ ts: number; clients: ClientTotals } | null>(null);
  useEffect(() => {
    if (!data) return;
    // Skip when the snapshot ts has not advanced — React Strict Mode
    // and parent re-renders can hand us the same data object twice in
    // a row, and the previous version recomputed a brand-new
    // ClientRates record on every such pass. That allocation pressure
    // showed up as a steady Chrome memory growth on the Clients tab.
    if (prevRef.current && prevRef.current.ts === data.ts) return;
    const cur: ClientTotals = {};
    for (const c of data.clients) {
      cur[c.client_id] = {
        queries: c.queries_total,
        transactions: c.transactions_total,
      };
    }
    const prev = prevRef.current;
    if (prev) {
      const dt = (data.ts - prev.ts) / 1000;
      if (dt > 0) {
        const next: ClientRates = {};
        for (const [id, totals] of Object.entries(cur)) {
          const p = prev.clients[id];
          if (p) {
            next[id] = {
              qps: Math.max(0, (totals.queries - p.queries) / dt),
              tps: Math.max(0, (totals.transactions - p.transactions) / dt),
            };
          }
        }
        setRates(next);
      }
    }
    prevRef.current = { ts: data.ts, clients: cur };
  }, [data]);
  return rates;
}

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

// Single labelled text input. Browsers turn the label into a click target for
// the field, and operators see the placeholder *and* the field name even
// after they start typing — placeholder-as-label loses the field name the
// moment you type one character.
function FilterField({
  label,
  value,
  onChange,
  width,
  mono,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  width: string;
  mono?: boolean;
}) {
  const id = `clients-filter-${label.replace(/\W+/g, "-")}`;
  return (
    <div className="flex flex-col">
      <label
        htmlFor={id}
        className="mb-0.5 text-[10px] uppercase tracking-wide text-text-dim"
      >
        {label}
      </label>
      <input
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className={`${width} rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text${mono ? " font-mono" : ""}`}
      />
    </div>
  );
}

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
  // Filters / sort / pagination live in the URL so an operator can
  // paste a triage link into Slack: /clients?pool=app@db&state=waiting
  // lands the teammate on the exact narrowed view.
  const [searchParams, setSearchParams] = useSearchParams();
  // Memoise so the object reference is stable across renders that did
  // not change a query param — otherwise downstream useMemo / useEffect
  // hooks that depend on `filters` re-fire on every parent render.
  const filters: Filters = useMemo(
    () => ({
      pool: searchParams.get("pool") ?? "",
      database: searchParams.get("database") ?? "",
      user: searchParams.get("user") ?? "",
      state: searchParams.get("state") ?? "",
      appName: searchParams.get("appName") ?? "",
      addr: searchParams.get("addr") ?? "",
    }),
    [searchParams],
  );
  const sort = (searchParams.get("sort") as SortKey) || "queries_total";
  const dir = (searchParams.get("dir") as SortDir) || "desc";
  const offset = Number(searchParams.get("offset") ?? "0") || 0;
  const writeParams = (mut: (sp: URLSearchParams) => void) => {
    const next = new URLSearchParams(searchParams);
    mut(next);
    setSearchParams(next, { replace: true });
  };
  const setFilters = (
    updater: Filters | ((prev: Filters) => Filters),
  ) => {
    const value =
      typeof updater === "function" ? updater(filters) : updater;
    writeParams((sp) => {
      const keys: (keyof Filters)[] = [
        "pool",
        "database",
        "user",
        "state",
        "appName",
        "addr",
      ];
      for (const k of keys) {
        if (value[k]) sp.set(k, value[k]);
        else sp.delete(k);
      }
      sp.delete("offset");
    });
  };
  const setSort = (v: SortKey) =>
    writeParams((sp) => {
      if (v !== "queries_total") sp.set("sort", v);
      else sp.delete("sort");
      sp.delete("offset");
    });
  const setDir = (next: SortDir | ((prev: SortDir) => SortDir)) => {
    const value = typeof next === "function" ? next(dir) : next;
    writeParams((sp) => {
      if (value !== "desc") sp.set("dir", value);
      else sp.delete("dir");
    });
  };
  const setOffset = (next: number | ((prev: number) => number)) => {
    const value = typeof next === "function" ? next(offset) : next;
    writeParams((sp) => {
      if (value > 0) sp.set("offset", String(value));
      else sp.delete("offset");
    });
  };

  const query = useMemo(() => buildQuery(filters, sort, dir, offset), [filters, sort, dir, offset]);
  const fetcher = useCallback(
    (signal: AbortSignal) => apiGet<ClientsDto>(`/api/clients?${query}`, authHeader, signal),
    [authHeader, query],
  );
  const poll = usePoll<ClientsDto>(fetcher, POLL_MS);
  const rates = useClientRates(poll.data);
  // Client-side sort over the visible page only. /api/clients still
  // paginates by lifetime counters (the server has no notion of qps), so
  // the operator sees "the 50 clients the server picked, re-ordered by
  // current rate". Tooltip on the header explains the scope.
  const [pageSort, setPageSort] = useState<"qps" | "tps" | null>(null);
  const [pageSortDir, setPageSortDir] = useState<SortDir>("desc");
  const onPageSort = (k: "qps" | "tps") => {
    if (pageSort === k) {
      setPageSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setPageSort(k);
      setPageSortDir("desc");
    }
  };
  const pageSortIndicator = (k: "qps" | "tps") =>
    pageSort === k ? (pageSortDir === "asc" ? " ▲" : " ▼") : "";
  const visibleClients = useMemo(() => {
    if (!poll.data) return [];
    if (!pageSort) return poll.data.clients;
    const arr = [...poll.data.clients];
    arr.sort((a, b) => {
      const av = rates[a.client_id]?.[pageSort] ?? 0;
      const bv = rates[b.client_id]?.[pageSort] ?? 0;
      return pageSortDir === "asc" ? av - bv : bv - av;
    });
    return arr;
  }, [poll.data, rates, pageSort, pageSortDir]);
  const filterActive =
    filters.pool !== "" ||
    filters.database !== "" ||
    filters.user !== "" ||
    filters.state !== "" ||
    filters.appName !== "" ||
    filters.addr !== "";
  const clearFilters = () => {
    setFilters({ pool: "", database: "", user: "", state: "", appName: "", addr: "" });
    setOffset(0);
  };

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
        help={{
          definition:
            "All active client sessions. Address-level search for a stuck or noisy session: sort by Q age ms to find frozen queries, by Age s for long-lived sessions, by Errors for the loud ones. State = waiting means the client is queued for a backend.",
          source: "SHOW CLIENTS",
          related: ["SHOW SERVERS", "pg_stat_activity.client_addr"],
          docsHref:
            "https://ozontech.github.io/pg_doorman/observability/admin-commands.html",
        }}
      />
      <div className="flex flex-wrap items-end gap-3 border-b border-border px-6 py-3">
        <FilterField label="pool" value={filters.pool} width="w-32"
          onChange={(v) => updateFilter("pool", v)} />
        <FilterField label="database" value={filters.database} width="w-32"
          onChange={(v) => updateFilter("database", v)} />
        <FilterField label="user" value={filters.user} width="w-32"
          onChange={(v) => updateFilter("user", v)} />
        <FilterField label="application_name" value={filters.appName} width="w-44"
          onChange={(v) => updateFilter("appName", v)} />
        <FilterField label="addr (ip:port substring)" value={filters.addr} width="w-56"
          mono onChange={(v) => updateFilter("addr", v)} />
        <div className="flex flex-col">
          <label className="mb-0.5 text-[10px] uppercase tracking-wide text-text-dim">state</label>
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
        </div>
        {filterActive && (
          <button
            type="button"
            onClick={clearFilters}
            className="border border-border-strong px-2 py-1 text-xs font-mono uppercase tracking-wider text-text-muted hover:text-accent"
            title="Clear all filters"
          >
            clear
          </button>
        )}
        <span className="ml-auto text-xs text-text-dim tabular">
          {showingFrom}–{showingTo} of {total}
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="Internal client id pg_doorman uses in logs and metrics. Format #cN; useful when grepping the LogTap.">
                Client
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="Peer ip:port. Empty for unix-socket connections. Cross-reference with pg_stat_activity.client_addr.">
                Addr
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="user@database the client is bound to. Click the row in Pools to drill down.">
                Pool
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="application_name the client sent in the startup parameters. Empty when the driver does not set it.">
                App
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="active = currently running a query / transaction. idle = checked in. waiting = queued for a backend.">
                State
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right" title="Why the client is waiting (e.g. lock, server) and for how long.">
              Wait
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Wall-clock age of the in-flight query in ms. 0 = no query in flight. > 30 000 ms = stuck query.">
                <span
                  className="cursor-pointer"
                  onClick={() => onSort("current_query_age_ms")}
                >
                  Q age ms{sortIndicator("current_query_age_ms")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Lifetime of the client connection in seconds since startup. Sort to find the oldest sessions during a leak hunt.">
                <span className="cursor-pointer" onClick={() => onSort("age_seconds")}>
                  Age s{sortIndicator("age_seconds")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Queries per second over the last poll interval (~1.5 s). Click to re-order the visible 50 rows by current rate; the server still paginates by lifetime queries, so this sort applies within the page only.">
                <span className="cursor-pointer" onClick={() => onPageSort("qps")}>
                  Q/s{pageSortIndicator("qps")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Transactions per second over the last poll interval. Click to re-order the visible 50 rows; useful next to Q/s to spot clients running many statements per transaction.">
                <span className="cursor-pointer" onClick={() => onPageSort("tps")}>
                  T/s{pageSortIndicator("tps")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Total queries this client has run since connection. Resets on reconnect.">
                <span className="cursor-pointer" onClick={() => onSort("queries_total")}>
                  Queries{sortIndicator("queries_total")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Total errors observed for this client. Sort to find the noisy ones — stuck transactions or auth retries.">
                <span className="cursor-pointer" onClick={() => onSort("errors_total")}>
                  Errors{sortIndicator("errors_total")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left" title="Client connected over TLS. Empty cell = plaintext on the loopback or on a trusted network.">TLS</th>
          </tr>
        </thead>
        <tbody>
          {visibleClients.map((c) => {
            const r = rates[c.client_id];
            return (
              <ClientRow
                key={c.client_id}
                client={c}
                qps={r?.qps ?? null}
                tps={r?.tps ?? null}
              />
            );
          })}
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

// Memoised row. /api/clients ships a fresh array every poll, so each `c`
// reference is new even when the client did not move; default React.memo
// shallow-compare would never skip. The custom comparator checks the
// fields that drive the render — anything else (e.g. tls flag flapping)
// is so rare it is fine to repaint when it changes.
const ClientRow = memo(
  function ClientRow({
    client: c,
    qps,
    tps,
  }: {
    client: ClientDto;
    qps: number | null;
    tps: number | null;
  }) {
    return (
      <tr className="border-b border-border hover:bg-surface-2">
        <td className="px-3 py-1.5 font-mono text-xs">{c.client_id}</td>
        <td className="px-3 py-1.5 font-mono text-xs text-text-muted">{c.addr || "—"}</td>
        <td className="px-3 py-1.5 text-xs">{c.user}@{c.database}</td>
        <td className="px-3 py-1.5 text-xs text-text-muted">{c.application_name || "—"}</td>
        <td className="px-3 py-1.5 text-xs">
          <span
            className={
              c.state === "active"
                ? "text-success"
                : c.state === "waiting"
                  ? "text-warning"
                  : "text-text-muted"
            }
          >
            {c.state}
          </span>
        </td>
        <td className="px-3 py-1.5 text-right text-xs text-text-muted">
          {c.wait && c.wait !== "none" ? (
            <span title={`reason: ${c.wait}`}>
              {c.wait_ms ? `${c.wait_ms} ms` : c.wait}
            </span>
          ) : (
            "—"
          )}
        </td>
        <td className="px-3 py-1.5 text-right">{c.current_query_age_ms || "—"}</td>
        <td className="px-3 py-1.5 text-right">{c.age_seconds}</td>
        <td className="px-3 py-1.5 text-right">
          {qps === null ? "—" : qps.toFixed(1)}
        </td>
        <td className="px-3 py-1.5 text-right">
          {tps === null ? "—" : tps.toFixed(1)}
        </td>
        <td className="px-3 py-1.5 text-right">{c.queries_total}</td>
        <td
          className={`px-3 py-1.5 text-right ${c.errors_total > 0 ? "text-warning" : ""}`}
        >
          {c.errors_total}
        </td>
        <td
          className="px-3 py-1.5 text-xs text-text-muted"
          title={c.tls ? "TLS" : "plaintext"}
        >
          {c.tls ? "✓" : ""}
        </td>
      </tr>
    );
  },
  (prev, next) =>
    prev.client.client_id === next.client.client_id &&
    prev.client.state === next.client.state &&
    prev.client.wait === next.client.wait &&
    prev.client.wait_ms === next.client.wait_ms &&
    prev.client.current_query_age_ms === next.client.current_query_age_ms &&
    prev.client.age_seconds === next.client.age_seconds &&
    prev.client.queries_total === next.client.queries_total &&
    prev.client.errors_total === next.client.errors_total &&
    prev.client.application_name === next.client.application_name &&
    prev.client.user === next.client.user &&
    prev.client.database === next.client.database &&
    prev.client.addr === next.client.addr &&
    prev.client.tls === next.client.tls &&
    prev.qps === next.qps &&
    prev.tps === next.tps,
);
