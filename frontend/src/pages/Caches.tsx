import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { apiGet } from "../api";
import { InfoLabel } from "../components/InfoLabel";
import { PageHero } from "../components/PageHero";
import { SectionHeader } from "../components/SectionHeader";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import { prettySql } from "../lib/prettySql";
import type {
  InternerDto,
  InternerTopDto,
  PreparedDto,
  PreparedRowDto,
  PreparedTextDto,
} from "../types";

const POLL_MS = 3000;

type Tab = "prepared" | "interner";

export default function Caches() {
  const [tab, setTab] = useState<Tab>("prepared");
  return (
    <section className="flex flex-col">
      <PageHero
        title="Caches"
        help={{
          definition:
            "Two caches whose miss rate converts directly into PostgreSQL CPU. Prepared — per-pool (hash → DOORMAN_N) statement cache; pg_doorman remaps anonymous Parse to a named statement so the backend re-uses the plan across clients. Query cache — process-wide SQL text interner (named + anonymous).",
          source:
            "SHOW PREPARED_STATEMENTS · SHOW INTERNER · SHOW INTERNER <N>",
          formula: "hit rate = hits / (hits + misses)",
          thresholds: {
            healthy: "hit rate ≥ 95 %",
            warn: "< 95 % — raise prepared_statements_cache_size",
            crit: "< 80 % — hot-path Parse on every call",
          },
          related: [
            "SHOW POOLS_MEMORY",
            "client_anonymous_prepared_cache_size",
          ],
          docsHref:
            "https://ozontech.github.io/pg_doorman/tutorials/prepared-statements.html",
        }}
      />
      <div className="flex items-center gap-1 border-b border-border bg-surface px-6">
        <TabButton active={tab === "prepared"} onClick={() => setTab("prepared")}>Prepared</TabButton>
        <TabButton active={tab === "interner"} onClick={() => setTab("interner")}>Query cache</TabButton>
      </div>
      {tab === "prepared" ? <PreparedTab /> : <InternerTab />}
    </section>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`px-4 py-3 text-sm border-b-2 ${
        active ? "border-accent text-text" : "border-transparent text-text-muted hover:text-text"
      }`}
    >
      {children}
    </button>
  );
}

interface TextCell {
  loading: boolean;
  text?: string;
  error?: string;
}

type PreparedSortKey =
  | "pool"
  | "kind"
  | "name"
  | "hash"
  | "count_used"
  | "hits"
  | "misses"
  | "hit_rate"
  | "refs_per_s";
type SortDir = "asc" | "desc";

type PreparedRefsTotals = Record<string, number>;
type PreparedRefsRates = Record<string, number>;

// Per-(pool, hash) refs/s — delta of (hits + misses) between consecutive
// /api/prepared snapshots, divided by the snapshot interval. The hits
// and misses counters bump on every Parse-time reference, so this rate
// shows which statements are touched right now versus which sit cold.
function usePreparedRefsRate(data: PreparedDto | null): PreparedRefsRates {
  const [rates, setRates] = useState<PreparedRefsRates>({});
  const prevRef = useRef<{ ts: number; totals: PreparedRefsTotals } | null>(null);
  useEffect(() => {
    if (!data) return;
    const totals: PreparedRefsTotals = {};
    for (const r of data.prepared) {
      totals[`${r.pool}|${r.hash}`] = r.hits + r.misses;
    }
    const prev = prevRef.current;
    if (prev && prev.ts !== data.ts) {
      const dt = (data.ts - prev.ts) / 1000;
      if (dt > 0) {
        const next: PreparedRefsRates = {};
        for (const [key, total] of Object.entries(totals)) {
          const p = prev.totals[key];
          if (p !== undefined) {
            next[key] = Math.max(0, (total - p) / dt);
          }
        }
        setRates(next);
      }
    }
    prevRef.current = { ts: data.ts, totals };
  }, [data]);
  return rates;
}

/// Hit-rate sentinel used to keep "no traffic yet" rows below real data
/// regardless of asc/desc — `hits + misses == 0` rows have nothing to
/// compare and dragging them to the top would bury the rows operators
/// actually care about.
const HIT_RATE_NO_DATA = -1;

function hitRateOrSentinel(r: PreparedRowDto): number {
  const total = r.hits + r.misses;
  return total > 0 ? r.hits / total : HIT_RATE_NO_DATA;
}

function comparePrepared(
  a: PreparedRowDto,
  b: PreparedRowDto,
  key: PreparedSortKey,
  rates: PreparedRefsRates,
): number {
  switch (key) {
    case "pool":
      return a.pool.localeCompare(b.pool);
    case "kind":
      return a.kind.localeCompare(b.kind);
    case "name":
      return (a.name || "").localeCompare(b.name || "");
    case "hash":
      return a.hash.localeCompare(b.hash);
    case "count_used":
      return a.count_used - b.count_used;
    case "hits":
      return a.hits - b.hits;
    case "misses":
      return a.misses - b.misses;
    case "hit_rate":
      return hitRateOrSentinel(a) - hitRateOrSentinel(b);
    case "refs_per_s":
      return (rates[`${a.pool}|${a.hash}`] ?? 0) - (rates[`${b.pool}|${b.hash}`] ?? 0);
  }
}

interface PreparedFilters {
  pool: string;
  name: string;
  hash: string;
  // "any" matches all kinds. The remaining values are exact matches against
  // the row's `kind` field as serialised by the backend.
  kind: "any" | "named" | "anonymous" | "mixed";
}

const EMPTY_FILTERS: PreparedFilters = { pool: "", name: "", hash: "", kind: "any" };

function matchesFilters(r: PreparedRowDto, f: PreparedFilters): boolean {
  if (f.pool && !r.pool.toLowerCase().includes(f.pool.toLowerCase())) return false;
  if (f.name && !(r.name || "").toLowerCase().includes(f.name.toLowerCase())) return false;
  if (f.hash && !r.hash.toLowerCase().includes(f.hash.toLowerCase())) return false;
  if (f.kind !== "any" && r.kind !== f.kind) return false;
  return true;
}

function PreparedTab() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<PreparedDto>(
    (signal) => apiGet<PreparedDto>("/api/prepared", authHeader, signal),
    POLL_MS,
  );
  // Lazy-loaded SQL text per (pool, hash). The /api/prepared response
  // omits the text on purpose (anonymous-safe public endpoint); admins
  // fetch it row-by-row via /api/prepared/text/{hash}.
  const [texts, setTexts] = useState<Record<string, TextCell>>({});
  const refsRate = usePreparedRefsRate(poll.data);
  // Default to "most-used statements first" — the question an operator
  // opens this page to answer is which statements drive cache pressure.
  const [sortKey, setSortKey] = useState<PreparedSortKey>("count_used");
  const [sortDir, setSortDir] = useState<SortDir>("desc");
  const [filters, setFilters] = useState<PreparedFilters>(EMPTY_FILTERS);
  const filterActive =
    filters.pool !== "" ||
    filters.name !== "" ||
    filters.hash !== "" ||
    filters.kind !== "any";
  const onSort = (k: PreparedSortKey) => {
    if (k === sortKey) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(k);
      setSortDir("desc");
    }
  };
  const sortIndicator = (k: PreparedSortKey) =>
    sortKey === k ? (sortDir === "asc" ? " ▲" : " ▼") : "";
  const sorted = useMemo(() => {
    if (!poll.data) return [];
    const arr = poll.data.prepared.filter((r) => matchesFilters(r, filters));
    arr.sort((a, b) => {
      const cmp = comparePrepared(a, b, sortKey, refsRate);
      return sortDir === "asc" ? cmp : -cmp;
    });
    return arr;
  }, [poll.data, filters, sortKey, sortDir, refsRate]);

  const toggle = (pool: string, hash: string) => {
    const key = `${pool}-${hash}`;
    setTexts((prev) => {
      const cur = prev[key];
      if (cur && (cur.text || cur.error)) {
        // Already loaded — collapse.
        const next = { ...prev };
        delete next[key];
        return next;
      }
      if (cur?.loading) return prev;
      return { ...prev, [key]: { loading: true } };
    });
    // Avoid double-fetching on re-toggle.
    if (texts[key]?.text || texts[key]?.error) return;
    apiGet<PreparedTextDto>(`/api/prepared/text/${hash}`, authHeader)
      .then((dto) => {
        setTexts((prev) => ({ ...prev, [key]: { loading: false, text: dto.query } }));
      })
      .catch((e: unknown) => {
        const msg = e instanceof Error ? e.message : String(e);
        setTexts((prev) => ({ ...prev, [key]: { loading: false, error: msg } }));
      });
  };

  if (poll.error) return <p className="p-4 text-sm text-danger">{poll.error.message}</p>;
  if (!poll.data) return <p className="p-4 text-sm text-text-dim">Loading prepared statements…</p>;

  return (
    <>
      <SectionHeader
        title="Prepared statements"
        help={{
          definition:
            "One row per (pool, prepared statement). Hits = parse-time hit on the server cache; misses = a fresh PostgreSQL Parse round-trip. Click a row to fetch the SQL body (admin-only, cached until collapse).",
          source: "SHOW PREPARED_STATEMENTS",
          formula: "hit rate = hits / (hits + misses)",
          thresholds: {
            healthy: "hit rate ≥ 95 % once pool is warm",
            warn: "< 95 % sustained — raise prepared_statements_cache_size",
            crit: "< 80 % — hot-path Parse on every call",
          },
          related: ["SHOW POOLS_MEMORY", "client_anonymous_prepared_cache_size"],
          docsHref:
            "https://ozontech.github.io/pg_doorman/tutorials/prepared-statements.html",
        }}
      />
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-3">
        <input
          placeholder="filter pool"
          value={filters.pool}
          onChange={(e) => setFilters((f) => ({ ...f, pool: e.target.value }))}
          className="w-44 rounded border border-border-strong bg-surface-2 px-2 py-1 font-mono text-xs text-text"
        />
        <input
          placeholder="filter name (DOORMAN_…)"
          value={filters.name}
          onChange={(e) => setFilters((f) => ({ ...f, name: e.target.value }))}
          className="w-56 rounded border border-border-strong bg-surface-2 px-2 py-1 font-mono text-xs text-text"
        />
        <input
          placeholder="filter hash"
          value={filters.hash}
          onChange={(e) => setFilters((f) => ({ ...f, hash: e.target.value }))}
          className="w-44 rounded border border-border-strong bg-surface-2 px-2 py-1 font-mono text-xs text-text"
        />
        <select
          value={filters.kind}
          onChange={(e) =>
            setFilters((f) => ({ ...f, kind: e.target.value as PreparedFilters["kind"] }))
          }
          className="rounded border border-border-strong bg-surface-2 px-2 py-1 font-mono text-xs text-text"
        >
          <option value="any">any kind</option>
          <option value="named">named</option>
          <option value="anonymous">anonymous</option>
          <option value="mixed">mixed</option>
        </select>
        {filterActive && (
          <button
            type="button"
            onClick={() => setFilters(EMPTY_FILTERS)}
            className="border border-border-strong px-2 py-1 text-xs font-mono uppercase tracking-wider text-text-muted hover:text-accent"
            title="Clear all filters"
          >
            clear
          </button>
        )}
        <span className="ml-auto text-xs text-text-dim tabular">
          {sorted.length} of {poll.data.prepared.length} statements
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="user@db pool that owns this cache entry. Each pool has its own prepared cache; the same SQL text in two pools shows up as two rows.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("pool")}>
                  Pool{sortIndicator("pool")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="How the client refers to this hash: named = explicit Parse with a non-empty statement name; anonymous = empty Parse name; mixed = both kinds were observed for this hash.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("kind")}>
                  Kind{sortIndicator("kind")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="Internal pg_doorman name (DOORMAN_N) assigned to the rewritten Parse. Used in PostgreSQL's pg_prepared_statements view on the backend side.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("name")}>
                  Name{sortIndicator("name")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-left">
              <InfoLabel tip="64-bit hash of the SQL text. Same SQL across pools and clients always produces the same hash — useful when correlating with /api/prepared/text/{hash}.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("hash")}>
                  Hash{sortIndicator("hash")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="LRU timestamp — bumped on every reference. Sort desc to find the most-used statements; the bottom of the asc sort is the eviction frontier.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("count_used")}>
                  Used{sortIndicator("count_used")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Parse-time references per second over the last poll interval — delta of (hits + misses) divided by the time gap. Sort desc to find the hot statements right now. Empty (—) until the second snapshot arrives.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("refs_per_s")}>
                  Refs/s{sortIndicator("refs_per_s")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Parse-time hits — the backend already had this prepared statement, so pg_doorman skipped the round-trip.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("hits")}>
                  Hits{sortIndicator("hits")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="Parse-time misses — pg_doorman had to send a fresh Parse to the backend before answering the client's Bind/Execute.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("misses")}>
                  Misses{sortIndicator("misses")}
                </span>
              </InfoLabel>
            </th>
            <th className="px-3 py-2 text-right">
              <InfoLabel tip="hits / (hits + misses). Healthy is ≥ 95 % once warm; below 80 % means most queries pay for a Parse on every call.">
                <span className="cursor-pointer hover:text-text" onClick={() => onSort("hit_rate")}>
                  Hit rate{sortIndicator("hit_rate")}
                </span>
              </InfoLabel>
            </th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((r) => {
            const total = r.hits + r.misses;
            const hitRate = total > 0 ? r.hits / total : null;
            const key = `${r.pool}-${r.hash}`;
            const cell = texts[key];
            return (
              <Fragment key={key}>
                <tr
                  className="cursor-pointer border-b border-border hover:bg-surface-2"
                  onClick={() => toggle(r.pool, r.hash)}
                  title="Click to fetch the SQL body"
                >
                  <td className="px-3 py-1.5 font-mono text-xs">{r.pool}</td>
                  <td className="px-3 py-1.5 text-xs text-text-muted">{r.kind}</td>
                  <td className="px-3 py-1.5 text-xs">{r.name || "—"}</td>
                  <td className="px-3 py-1.5 font-mono text-xs text-text-dim">{r.hash}</td>
                  <td className="px-3 py-1.5 text-right">{r.count_used}</td>
                  <td className="px-3 py-1.5 text-right">
                    {(() => {
                      const v = refsRate[`${r.pool}|${r.hash}`];
                      return v === undefined ? "—" : v.toFixed(1);
                    })()}
                  </td>
                  <td className="px-3 py-1.5 text-right">{r.hits}</td>
                  <td className="px-3 py-1.5 text-right">{r.misses}</td>
                  <td className={`px-3 py-1.5 text-right ${
                    hitRate !== null && hitRate < 0.8 ? "text-danger" :
                    hitRate !== null && hitRate < 0.95 ? "text-warning" : ""
                  }`}>
                    {hitRate === null ? "—" : `${(hitRate * 100).toFixed(1)}%`}
                  </td>
                </tr>
                {cell && (
                  <tr className="border-b border-border bg-surface-2">
                    <td colSpan={9} className="px-4 py-3">
                      {cell.loading && <span className="text-xs text-text-dim">loading SQL…</span>}
                      {cell.error && (
                        <span className="text-xs text-danger">SQL fetch failed: {cell.error}</span>
                      )}
                      {cell.text && (
                        <div className="space-y-2">
                          <div className="flex items-center justify-between text-[10px] uppercase tracking-wide text-text-dim">
                            <span>SQL · {key}</span>
                            <button
                              type="button"
                              className="border border-border-strong px-2 py-0.5 text-text-muted hover:text-accent"
                              onClick={() => navigator.clipboard?.writeText(cell.text!)}
                              title="Copy raw SQL to clipboard"
                            >
                              copy
                            </button>
                          </div>
                          <pre className="overflow-x-auto whitespace-pre border border-border bg-bg p-3 font-mono text-xs leading-relaxed text-text">
{prettySql(cell.text)}
                          </pre>
                        </div>
                      )}
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
      {poll.data.prepared.length === 0 && (
        <p className="p-4 text-sm text-text-dim">No prepared statements yet. The cache fills as clients send Parse over the wire — open the Clients page to confirm traffic is flowing.</p>
      )}
      {poll.data.prepared.length > 0 && sorted.length === 0 && (
        <p className="p-4 text-sm text-text-dim">No statements match these filters. Click clear to see them all again.</p>
      )}
    </>
  );
}

function InternerTab() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<InternerDto>(
    (signal) => apiGet<InternerDto>("/api/interner", authHeader, signal),
    POLL_MS,
  );
  // Admin-only top-N from /api/interner/top — needed to show *which*
  // entries dominate the cache. Without it the tab is just two summary
  // cards and offers no actionable information.
  const topPoll = usePoll<InternerTopDto>(
    (signal) => apiGet<InternerTopDto>("/api/interner/top?n=20", authHeader, signal),
    POLL_MS,
  );

  if (poll.error) return <p className="p-4 text-sm text-danger">{poll.error.message}</p>;
  if (!poll.data) return <p className="p-4 text-sm text-text-dim">Loading interner stats…</p>;

  const fmtBytes = (n: number) => {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
    return `${(n / 1024 / 1024).toFixed(2)} MiB`;
  };

  return (
    <>
      <SectionHeader
        title="Query interner"
        help={{
          definition:
            "Process-wide byte-deduplicated SQL text cache. Named = explicitly prepared statements; anonymous = ad-hoc queries with no name (Parse with empty name).",
          source: "SHOW INTERNER",
          related: ["SHOW INTERNER <N>", "client_anonymous_prepared_cache_size"],
          thresholds: {
            healthy: "named bytes flat",
            warn: "anonymous bytes climbing without bound",
            crit: "anonymous bytes near memory cap — app sending unique SQL each call",
          },
          docsHref:
            "https://ozontech.github.io/pg_doorman/operations/monitoring-interner.html",
        }}
      />
      <div className="grid grid-cols-2 gap-6 p-6">
        <Card title="Named" entries={poll.data.named.entries} bytes={poll.data.named.bytes} fmtBytes={fmtBytes} />
        <Card title="Anonymous" entries={poll.data.anonymous.entries} bytes={poll.data.anonymous.bytes} fmtBytes={fmtBytes} />
      </div>
      <SectionHeader
        title="Top entries by bytes"
        help={{
          definition:
            "Twenty largest interned SQL texts across both kinds. Outliers here are usually one-off statements that should be parametrised.",
          source: "SHOW INTERNER 20",
          related: ["SHOW INTERNER"],
          docsHref:
            "https://ozontech.github.io/pg_doorman/operations/monitoring-interner.html",
        }}
      />
      {topPoll.error && (
        <p className="px-6 pb-4 text-sm text-danger">{topPoll.error.message}</p>
      )}
      {topPoll.data && (
        <div className="px-6 pb-6">
          <table className="w-full text-sm tabular">
            <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
              <tr>
                <th className="px-3 py-2 text-left">
                  <InfoLabel tip="64-bit hash of the SQL text. Same SQL always hashes the same way — paste into the Prepared tab's hash filter to see which pools cache it.">
                    Hash
                  </InfoLabel>
                </th>
                <th className="px-3 py-2 text-left">
                  <InfoLabel tip="Which interner half holds this entry: named = prepared statements; anonymous = ad-hoc SQL.">
                    Kind
                  </InfoLabel>
                </th>
                <th className="px-3 py-2 text-right">
                  <InfoLabel tip="Bytes the SQL text occupies in the interner. Server has Top 20 sorted desc by this column already.">
                    Bytes
                  </InfoLabel>
                </th>
                <th className="px-3 py-2 text-right">
                  <InfoLabel tip="Wall time in ms since this entry was last touched (anonymous side only — named entries never go idle, the column shows '—').">
                    Idle ms
                  </InfoLabel>
                </th>
                <th className="px-3 py-2 text-left">
                  <InfoLabel tip="First ~120 characters of the SQL, trimmed at a UTF-8 boundary. Click the matching row in the Prepared tab to fetch the full statement.">
                    Preview
                  </InfoLabel>
                </th>
              </tr>
            </thead>
            <tbody>
              {topPoll.data.entries.map((e) => (
                <tr key={e.hash} className="border-b border-border hover:bg-surface-2">
                  <td className="px-3 py-1.5 font-mono text-xs text-text-dim">{e.hash}</td>
                  <td className="px-3 py-1.5 text-xs text-text-muted">{e.kind}</td>
                  <td className="px-3 py-1.5 text-right">{fmtBytes(e.bytes)}</td>
                  <td className="px-3 py-1.5 text-right text-xs text-text-muted">
                    {e.idle_ms < 0 ? "—" : e.idle_ms}
                  </td>
                  <td className="px-3 py-1.5 font-mono text-xs">{e.preview}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {topPoll.data.entries.length === 0 && (
            <p className="p-4 text-sm text-text-dim">Interner is empty. Either no SQL has been seen yet, or the build was compiled without the interner.</p>
          )}
        </div>
      )}
    </>
  );
}

function Card({
  title,
  entries,
  bytes,
  fmtBytes,
}: {
  title: string;
  entries: number;
  bytes: number;
  fmtBytes: (n: number) => string;
}) {
  return (
    <div className="rounded border border-border bg-surface p-4">
      <h3 className="mb-3 text-sm font-semibold text-text">{title}</h3>
      <dl className="grid grid-cols-2 gap-y-2 text-sm tabular">
        <dt className="text-text-muted">
          <InfoLabel tip="Live count of unique SQL texts held by this interner half. Bounded by passive GC (named) or per-entry TTL (anonymous).">
            Entries
          </InfoLabel>
        </dt>
        <dd className="text-right">{entries}</dd>
        <dt className="text-text-muted">
          <InfoLabel tip="Sum of every interned text's byte length plus per-entry overhead. Shown alongside RSS in /api/process/memory under interner.">
            Total bytes
          </InfoLabel>
        </dt>
        <dd className="text-right">{fmtBytes(bytes)}</dd>
        <dt className="text-text-muted">
          <InfoLabel tip="Total bytes / entries. A useful proxy for SQL shape — small avg = many short statements; large avg = a few big DML / DDL strings dominate.">
            Avg bytes / entry
          </InfoLabel>
        </dt>
        <dd className="text-right">{entries > 0 ? fmtBytes(Math.round(bytes / entries)) : "—"}</dd>
      </dl>
    </div>
  );
}
