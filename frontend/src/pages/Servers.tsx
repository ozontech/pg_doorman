import { useCallback, useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { apiGet } from "../api";
import { PageHero } from "../components/PageHero";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import { fmtBytes, fmtMs } from "../lib/format";
import type { ServersDto } from "../types";

const POLL_MS = 3000;
const PAGE_SIZE = 50;

interface Filters {
  database: string;
  user: string;
  state: string;
  application_name: string;
}

const STATE_OPTIONS = ["", "active", "idle", "used", "login"];

export default function Servers() {
  const { authHeader } = useAdminAuth();
  const [searchParams, setSearchParams] = useSearchParams();
  const filters: Filters = useMemo(
    () => ({
      database: searchParams.get("database") ?? "",
      user: searchParams.get("user") ?? "",
      state: searchParams.get("state") ?? "",
      application_name: searchParams.get("application_name") ?? "",
    }),
    [searchParams],
  );
  const offset = Number(searchParams.get("offset") ?? "0") || 0;
  const writeParams = (mut: (sp: URLSearchParams) => void) => {
    const next = new URLSearchParams(searchParams);
    mut(next);
    setSearchParams(next, { replace: true });
  };
  const updateFilter = (k: keyof Filters, v: string) =>
    writeParams((sp) => {
      if (v) sp.set(k, v);
      else sp.delete(k);
      sp.delete("offset");
    });
  const setOffset = (v: number) =>
    writeParams((sp) => (v > 0 ? sp.set("offset", String(v)) : sp.delete("offset")));

  const query = useMemo(() => {
    const sp = new URLSearchParams();
    sp.set("limit", String(PAGE_SIZE));
    sp.set("offset", String(offset));
    for (const [k, v] of Object.entries(filters)) {
      if (v) sp.set(k, v);
    }
    return sp.toString();
  }, [filters, offset]);
  const fetcher = useCallback(
    (signal: AbortSignal) =>
      apiGet<ServersDto>(`/api/servers?${query}`, authHeader, signal),
    [authHeader, query],
  );
  const poll = usePoll<ServersDto>(fetcher, POLL_MS);

  const total = poll.data?.total ?? 0;
  const showingFrom = total === 0 ? 0 : offset + 1;
  const showingTo = Math.min(total, offset + (poll.data?.servers.length ?? 0));
  const canPrev = offset > 0;
  const canNext = offset + PAGE_SIZE < total;

  if (poll.error) {
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Servers</h1>
        <p className="mt-2 text-sm text-danger">
          Could not load servers: {poll.error.message}.
        </p>
      </section>
    );
  }

  return (
    <section className="flex flex-col">
      <PageHero
        title="Servers"
        help={{
          definition:
            "All backend PostgreSQL connections the pooler currently holds. Pair with Clients to map a stuck query (#cN) to the backend pid that runs it (server_id → process_id). bytes_sent/recv shows which backend is moving traffic.",
          source: "SHOW SERVERS",
          related: ["SHOW CLIENTS", "pg_stat_activity.pid"],
          docsHref:
            "https://ozontech.github.io/pg_doorman/observability/admin-commands.html",
        }}
      />
      <div className="flex flex-wrap items-end gap-3 border-b border-border px-6 py-3">
        <FilterField label="database" value={filters.database} onChange={(v) => updateFilter("database", v)} />
        <FilterField label="user" value={filters.user} onChange={(v) => updateFilter("user", v)} />
        <FilterField label="application_name" value={filters.application_name} onChange={(v) => updateFilter("application_name", v)} width="w-48" />
        <div className="flex flex-col">
          <label className="mb-0.5 text-xs text-text-dim">state</label>
          <select
            value={filters.state}
            onChange={(e) => updateFilter("state", e.target.value)}
            className="rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
          >
            {STATE_OPTIONS.map((s) => (
              <option key={s} value={s}>
                {s || "all states"}
              </option>
            ))}
          </select>
        </div>
        <span className="ml-auto text-xs text-text-dim tabular">
          {total === 0 ? "no servers" : `${showingFrom}–${showingTo} of ${total}`}
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-xs uppercase tracking-wide text-text-muted">
          <tr>
            <th className="px-3 py-2 text-right">server_id</th>
            <th className="px-3 py-2 text-right">pid</th>
            <th className="px-3 py-2 text-left">db / user</th>
            <th className="px-3 py-2 text-left">app</th>
            <th className="px-3 py-2 text-left">state</th>
            <th className="px-3 py-2 text-right">active age</th>
            <th className="px-3 py-2 text-right">age s</th>
            <th className="px-3 py-2 text-right">queries</th>
            <th className="px-3 py-2 text-right">errors</th>
            <th className="px-3 py-2 text-right">bytes sent</th>
            <th className="px-3 py-2 text-right">bytes recv</th>
            <th className="px-3 py-2 text-left">tls</th>
          </tr>
        </thead>
        <tbody>
          {poll.data?.servers.map((s) => (
            <tr key={s.server_id} className="border-b border-border hover:bg-surface-2">
              <td className="px-3 py-1.5 font-mono text-xs">{s.server_id}</td>
              <td className="px-3 py-1.5 font-mono text-xs">{s.process_id}</td>
              <td className="px-3 py-1.5 text-xs">{s.user}@{s.database}</td>
              <td className="px-3 py-1.5 text-xs text-text-muted">{s.application_name || "—"}</td>
              <td className="px-3 py-1.5 text-xs">
                <span
                  className={
                    s.state === "active"
                      ? "text-success"
                      : s.state === "login"
                        ? "text-warning"
                        : "text-text-muted"
                  }
                >
                  {s.state}
                </span>
              </td>
              <td className={`px-3 py-1.5 text-right ${s.active_age_ms > 30_000 ? "text-warning" : ""} ${s.active_age_ms > 300_000 ? "text-danger" : ""}`}>
                {s.active_age_ms > 0 ? fmtMs(s.active_age_ms) : "—"}
              </td>
              <td className="px-3 py-1.5 text-right">{s.age_seconds}</td>
              <td className="px-3 py-1.5 text-right">{s.queries_total}</td>
              <td className={`px-3 py-1.5 text-right ${s.errors_total > 0 ? "text-warning" : ""}`}>{s.errors_total}</td>
              <td className="px-3 py-1.5 text-right text-text-muted">{fmtBytes(s.bytes_sent)}</td>
              <td className="px-3 py-1.5 text-right text-text-muted">{fmtBytes(s.bytes_received)}</td>
              <td className="px-3 py-1.5 text-xs text-text-muted">{s.tls ? "✓" : ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="flex items-center gap-2 px-4 py-3">
        <button
          type="button"
          disabled={!canPrev}
          onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
          className="rounded border border-border-strong bg-surface-2 px-3 py-1 text-sm text-text disabled:opacity-40"
        >
          ← prev
        </button>
        <button
          type="button"
          disabled={!canNext}
          onClick={() => setOffset(offset + PAGE_SIZE)}
          className="rounded border border-border-strong bg-surface-2 px-3 py-1 text-sm text-text disabled:opacity-40"
        >
          next →
        </button>
        {!poll.data && <span className="text-sm text-text-dim">loading…</span>}
      </div>
    </section>
  );
}

function FilterField({
  label,
  value,
  onChange,
  width = "w-32",
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  width?: string;
}) {
  const id = `srv-${label}`;
  return (
    <div className="flex flex-col">
      <label htmlFor={id} className="mb-0.5 text-xs text-text-dim">
        {label}
      </label>
      <input
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className={`${width} rounded border border-border-strong bg-surface-2 px-2 py-1 font-mono text-sm text-text`}
      />
    </div>
  );
}
