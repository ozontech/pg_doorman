import { Fragment, useState, type ReactNode } from "react";
import { apiGet } from "../api";
import { PageHero } from "../components/PageHero";
import { SectionHeader } from "../components/SectionHeader";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { InternerDto, InternerTopDto, PreparedDto, PreparedTextDto } from "../types";

const POLL_MS = 3000;

type Tab = "prepared" | "interner";

export default function Caches() {
  const [tab, setTab] = useState<Tab>("prepared");
  return (
    <section className="flex flex-col">
      <PageHero
        title="Caches"
        description="Two backend caches that affect connection efficiency: per-pool prepared statements and the global query interner that deduplicates SQL text."
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
  if (!poll.data) return <p className="p-4 text-sm text-text-dim">loading…</p>;

  return (
    <>
      <SectionHeader
        title="Prepared statements"
        what="One row per (pool, prepared statement). Hits = parse-time hit on the server cache; misses = a fresh PostgreSQL Parse round-trip. Click a row to fetch the SQL body."
        how="Polled every 3 s from /api/prepared. The SQL text comes from the admin-only /api/prepared/text/{hash} endpoint and stays cached client-side until you collapse the row."
        normal="Hit rate ≥ 95 % once warm. Amber under 95 %, red under 80 % — bump prepared-statement-cache size if sustained."
      />
      <div className="overflow-x-auto">
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-3 py-2 text-left">Pool</th>
            <th className="px-3 py-2 text-left">Kind</th>
            <th className="px-3 py-2 text-left">Name</th>
            <th className="px-3 py-2 text-left">Hash</th>
            <th className="px-3 py-2 text-right">Used</th>
            <th className="px-3 py-2 text-right">Hits</th>
            <th className="px-3 py-2 text-right">Misses</th>
            <th className="px-3 py-2 text-right">Hit rate</th>
          </tr>
        </thead>
        <tbody>
          {poll.data.prepared.map((r) => {
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
                    <td colSpan={8} className="px-4 py-2">
                      {cell.loading && <span className="text-xs text-text-dim">loading SQL…</span>}
                      {cell.error && (
                        <span className="text-xs text-danger">SQL fetch failed: {cell.error}</span>
                      )}
                      {cell.text && (
                        <pre className="whitespace-pre-wrap break-all font-mono text-xs text-text">
{cell.text}
                        </pre>
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
        <p className="p-4 text-sm text-text-dim">No prepared statements cached yet.</p>
      )}
      </div>
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
  if (!poll.data) return <p className="p-4 text-sm text-text-dim">loading…</p>;

  const fmtBytes = (n: number) => {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
    return `${(n / 1024 / 1024).toFixed(2)} MiB`;
  };

  return (
    <>
      <SectionHeader
        title="Query interner"
        what="Global byte-deduplicated SQL text cache. Named = explicitly prepared statements; anonymous = ad-hoc queries."
        how="Polled every 3 s from /api/interner."
        normal="Anonymous bytes growing without bound = lower client_anonymous_prepared_cache_size or shorten anon idle TTL."
      />
      <div className="grid grid-cols-2 gap-6 p-6">
        <Card title="Named" entries={poll.data.named.entries} bytes={poll.data.named.bytes} fmtBytes={fmtBytes} />
        <Card title="Anonymous" entries={poll.data.anonymous.entries} bytes={poll.data.anonymous.bytes} fmtBytes={fmtBytes} />
      </div>
      <SectionHeader
        title="Top entries by bytes"
        what="Largest interned SQL texts across both kinds. Useful for spotting outlier statements that bloat the cache."
        how="Admin-only /api/interner/top?n=20. The preview is the first 120 characters of the SQL text, truncated to keep multi-byte sequences whole."
      />
      {topPoll.error && (
        <p className="px-6 pb-4 text-sm text-danger">{topPoll.error.message}</p>
      )}
      {topPoll.data && (
        <div className="overflow-x-auto px-6 pb-6">
          <table className="w-full text-sm tabular">
            <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
              <tr>
                <th className="px-3 py-2 text-left">Hash</th>
                <th className="px-3 py-2 text-left">Kind</th>
                <th className="px-3 py-2 text-right">Bytes</th>
                <th className="px-3 py-2 text-right">Idle ms</th>
                <th className="px-3 py-2 text-left">Preview</th>
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
            <p className="p-4 text-sm text-text-dim">No interned entries yet.</p>
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
        <dt className="text-text-muted">Entries</dt>
        <dd className="text-right">{entries}</dd>
        <dt className="text-text-muted">Total bytes</dt>
        <dd className="text-right">{fmtBytes(bytes)}</dd>
        <dt className="text-text-muted">Avg bytes / entry</dt>
        <dd className="text-right">{entries > 0 ? fmtBytes(Math.round(bytes / entries)) : "—"}</dd>
      </dl>
    </div>
  );
}
