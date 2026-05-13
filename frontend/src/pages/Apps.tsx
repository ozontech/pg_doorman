// /api/apps already aggregated client counters by `application_name` on the
// backend; the JSON DTO has been there since phase 3d-1 but no frontend
// page rendered it. This page adds a sortable application-level view instead
// of making operators filter Clients by application_name.

import { useEffect, useMemo, useRef, useState } from "react";
import { apiGet } from "../api";
import { InfoLabel } from "../components/InfoLabel";
import { PageHero } from "../components/PageHero";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { AppsDto } from "../types";

const POLL_MS = 1500;

type SortKey =
  | "application_name"
  | "clients"
  | "qps"
  | "tps"
  | "queries_total"
  | "transactions_total"
  | "errors_total";
type SortDir = "asc" | "desc";

type AppTotals = Record<string, { queries: number; transactions: number }>;
type AppRates = Record<string, { qps: number; tps: number }>;

// Computes per-application qps / tps from the delta between the current
// `/api/apps` snapshot and the previous one. The endpoint only ships
// cumulative counters, so the rate is derived in the browser.
function useAppRates(data: AppsDto | null): AppRates {
  const [rates, setRates] = useState<AppRates>({});
  const prevRef = useRef<{ ts: number; apps: AppTotals } | null>(null);
  useEffect(() => {
    if (!data) return;
    const cur: AppTotals = {};
    for (const a of data.apps) {
      cur[a.application_name] = {
        queries: a.queries_total,
        transactions: a.transactions_total,
      };
    }
    const prev = prevRef.current;
    if (prev && prev.ts !== data.ts) {
      const dt = (data.ts - prev.ts) / 1000;
      if (dt > 0) {
        const next: AppRates = {};
        for (const [name, totals] of Object.entries(cur)) {
          const p = prev.apps[name];
          if (p) {
            next[name] = {
              qps: Math.max(0, (totals.queries - p.queries) / dt),
              tps: Math.max(0, (totals.transactions - p.transactions) / dt),
            };
          }
        }
        setRates(next);
      }
    }
    prevRef.current = { ts: data.ts, apps: cur };
  }, [data]);
  return rates;
}

export default function Apps() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<AppsDto>(
    (signal) => apiGet<AppsDto>("/api/apps", authHeader, signal),
    POLL_MS,
  );
  const rates = useAppRates(poll.data);
  const [filter, setFilter] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("qps");
  const [sortDir, setSortDir] = useState<SortDir>("desc");

  const rows = useMemo(() => {
    if (!poll.data) return [];
    const flt = filter.trim().toLowerCase();
    let list = poll.data.apps;
    if (flt) list = list.filter((r) => r.application_name.toLowerCase().includes(flt));
    list = list.slice().sort((a, b) => {
      const dir = sortDir === "asc" ? 1 : -1;
      switch (sortKey) {
        case "application_name":
          return a.application_name.localeCompare(b.application_name) * dir;
        case "clients":
          return (a.clients - b.clients) * dir;
        case "qps":
          return ((rates[a.application_name]?.qps ?? 0) - (rates[b.application_name]?.qps ?? 0)) * dir;
        case "tps":
          return ((rates[a.application_name]?.tps ?? 0) - (rates[b.application_name]?.tps ?? 0)) * dir;
        case "queries_total":
          return (a.queries_total - b.queries_total) * dir;
        case "transactions_total":
          return (a.transactions_total - b.transactions_total) * dir;
        case "errors_total":
          return (a.errors_total - b.errors_total) * dir;
      }
    });
    return list;
  }, [poll.data, rates, filter, sortKey, sortDir]);

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir(key === "application_name" ? "asc" : "desc");
    }
  };
  const sortIndicator = (key: SortKey) =>
    sortKey === key ? (sortDir === "asc" ? " ▲" : " ▼") : "";

  if (poll.error) {
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Apps</h1>
        <p className="mt-2 text-sm text-danger">
          Could not load apps: {poll.error.message}. Try Sign out → Sign in to refresh credentials, or check whether pg_doorman is running.
        </p>
      </section>
    );
  }

  return (
    <section className="flex flex-col">
      <PageHero
        title="Apps"
        help={{
          definition:
            "One row per application_name from the libpq StartupMessage. Use it to see which applications generate traffic and errors. Sort and filter run in the browser on the latest snapshot.",
          source: "derived from SHOW CLIENTS (group by application_name)",
          formula: "err / 1k q = errors_total × 1000 / queries_total",
          thresholds: {
            healthy: "err / 1k q < 1",
            warn: "1–10",
            crit: "> 10 — inspect the app's recent deploy",
          },
          related: ["SHOW STATS", "pg_stat_activity.application_name"],
          docsHref:
            "https://ozontech.github.io/pg_doorman/observability/admin-commands.html",
        }}
      />
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-6 py-3">
        <input
          placeholder="filter application_name"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="w-64 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text font-mono"
        />
        <span className="ml-auto text-xs text-text-dim tabular">
          {rows.length} app{rows.length === 1 ? "" : "s"}
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-3 py-2 text-left">
              <span
                className="cursor-pointer"
                onClick={() => onSort("application_name")}
              >
                application_name{sortIndicator("application_name")}
              </span>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Currently-connected clients with this application_name.">
                <span className="cursor-pointer" onClick={() => onSort("clients")}>
                  clients{sortIndicator("clients")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Queries per second over the last poll interval (~1.5 s). Sort to find the highest-traffic app right now. Empty on the first tick; rate needs two snapshots.">
                <span className="cursor-pointer" onClick={() => onSort("qps")}>
                  qps{sortIndicator("qps")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Transactions per second over the last poll interval. Compare against qps to spot apps doing many statements per transaction.">
                <span className="cursor-pointer" onClick={() => onSort("tps")}>
                  tx/s{sortIndicator("tps")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Total queries from this app since pg_doorman started.">
                <span className="cursor-pointer" onClick={() => onSort("queries_total")}>
                  queries{sortIndicator("queries_total")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Total transactions from this app since pg_doorman started.">
                <span
                  className="cursor-pointer"
                  onClick={() => onSort("transactions_total")}
                >
                  transactions{sortIndicator("transactions_total")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Total errors observed from this app's clients.">
                <span className="cursor-pointer" onClick={() => onSort("errors_total")}>
                  errors{sortIndicator("errors_total")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="errors / queries × 1000. Above 1 is unusual; above 10 means the app's recent deploy is a good first check.">
                err / 1k q
              </InfoLabel>
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const errPerK = r.queries_total > 0 ? (r.errors_total * 1000) / r.queries_total : 0;
            const errTone =
              errPerK > 10
                ? "text-danger"
                : errPerK > 1
                  ? "text-warning"
                  : "text-text-muted";
            return (
              <tr
                key={r.application_name || "(unknown)"}
                className="border-b border-border hover:bg-surface-2"
              >
                <td className="px-3 py-1.5 font-mono text-xs">
                  {r.application_name || <span className="text-text-dim">(unknown)</span>}
                </td>
                <td className="px-3 py-1.5 text-right">{r.clients}</td>
                <td className="px-3 py-1.5 text-right">
                  {rates[r.application_name]
                    ? rates[r.application_name].qps.toFixed(1)
                    : "—"}
                </td>
                <td className="px-3 py-1.5 text-right">
                  {rates[r.application_name]
                    ? rates[r.application_name].tps.toFixed(1)
                    : "—"}
                </td>
                <td className="px-3 py-1.5 text-right">{r.queries_total.toLocaleString()}</td>
                <td className="px-3 py-1.5 text-right">{r.transactions_total.toLocaleString()}</td>
                <td
                  className={`px-3 py-1.5 text-right ${
                    r.errors_total > 0 ? "text-warning" : ""
                  }`}
                >
                  {r.errors_total.toLocaleString()}
                </td>
                <td className={`px-3 py-1.5 text-right ${errTone}`}>{errPerK.toFixed(2)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {!poll.data && <p className="px-4 py-4 text-sm text-text-dim">Loading apps…</p>}
      {poll.data && rows.length === 0 && (
        <p className="px-4 py-4 text-sm text-text-dim">No application_name matches that fragment. Try a shorter or different substring.</p>
      )}
    </section>
  );
}
