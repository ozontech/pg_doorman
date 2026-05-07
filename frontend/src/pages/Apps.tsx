// /api/apps already aggregated client counters by `application_name` on the
// backend; the JSON DTO has been there since phase 3d-1 but no frontend
// page rendered it. This file fixes that — operators looking for "which
// app holds 30 connections / generates the error spike / churns reconnects"
// now have a single sortable table instead of grepping the Clients view by
// application_name substring.

import { useMemo, useState } from "react";
import { apiGet } from "../api";
import { PageHero } from "../components/PageHero";
import { SectionHeader } from "../components/SectionHeader";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { AppsDto } from "../types";

const POLL_MS = 1500;

type SortKey = "application_name" | "clients" | "queries_total" | "transactions_total" | "errors_total";
type SortDir = "asc" | "desc";

export default function Apps() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<AppsDto>(
    (signal) => apiGet<AppsDto>("/api/apps", authHeader, signal),
    POLL_MS,
  );
  const [filter, setFilter] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("clients");
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
        case "queries_total":
          return (a.queries_total - b.queries_total) * dir;
        case "transactions_total":
          return (a.transactions_total - b.transactions_total) * dir;
        case "errors_total":
          return (a.errors_total - b.errors_total) * dir;
      }
    });
    return list;
  }, [poll.data, filter, sortKey, sortDir]);

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
        description="One row per application_name as reported by clients. clients = currently connected; the totals are cumulative since pg_doorman started. Sort by err/1k q to find apps that fail more often per request — values above 1 are unusual; above 10 means time to look at the app's recent deploy."
      />
      <SectionHeader
        title="Aggregates"
        what="One row per application_name. clients = currently-connected; the totals are cumulative since the pooler started."
        how="Sort and filter happen in the browser, on the latest snapshot — no extra request per keystroke."
        normal="When err/1k q jumps for one app while the rest stay flat, that app is the suspect — start by asking who deployed last."
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
            <th
              className="cursor-pointer px-3 py-2 text-left"
              onClick={() => onSort("application_name")}
              title="application_name as reported by the client at startup (libpq, JDBC, etc.). Use to identify which app is misbehaving."
            >
              application_name{sortIndicator("application_name")}
            </th>
            <th
              className="cursor-pointer px-3 py-2 text-right"
              onClick={() => onSort("clients")}
              title="Currently-connected clients with this application_name."
            >
              clients{sortIndicator("clients")}
            </th>
            <th
              className="cursor-pointer px-3 py-2 text-right"
              onClick={() => onSort("queries_total")}
              title="Total queries from this app since pg_doorman started."
            >
              queries{sortIndicator("queries_total")}
            </th>
            <th
              className="cursor-pointer px-3 py-2 text-right"
              onClick={() => onSort("transactions_total")}
              title="Total transactions from this app since pg_doorman started."
            >
              transactions{sortIndicator("transactions_total")}
            </th>
            <th
              className="cursor-pointer px-3 py-2 text-right"
              onClick={() => onSort("errors_total")}
              title="Total errors observed from this app's clients."
            >
              errors{sortIndicator("errors_total")}
            </th>
            <th
              className="px-3 py-2 text-right"
              title="errors / queries × 1000. Above 1 is unusual; above 10 = look at the app's recent deploy."
            >
              err / 1k q
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
