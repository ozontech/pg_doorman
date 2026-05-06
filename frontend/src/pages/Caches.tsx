import { useState, type ReactNode } from "react";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { InternerDto, PreparedDto } from "../types";

const POLL_MS = 3000;

type Tab = "prepared" | "interner";

export default function Caches() {
  const [tab, setTab] = useState<Tab>("prepared");
  return (
    <section className="flex flex-col">
      <div className="flex items-center gap-1 border-b border-border bg-surface px-4">
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

function PreparedTab() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<PreparedDto>(
    (signal) => apiGet<PreparedDto>("/api/prepared", authHeader, signal),
    POLL_MS,
  );

  if (poll.error) return <p className="p-4 text-sm text-danger">{poll.error.message}</p>;
  if (!poll.data) return <p className="p-4 text-sm text-text-dim">loading…</p>;

  return (
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
            return (
              <tr key={`${r.pool}-${r.hash}`} className="border-b border-border hover:bg-surface-2">
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
            );
          })}
        </tbody>
      </table>
      {poll.data.prepared.length === 0 && (
        <p className="p-4 text-sm text-text-dim">No prepared statements cached yet.</p>
      )}
    </div>
  );
}

function InternerTab() {
  const { authHeader } = useAdminAuth();
  const poll = usePoll<InternerDto>(
    (signal) => apiGet<InternerDto>("/api/interner", authHeader, signal),
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
    <div className="grid grid-cols-2 gap-6 p-6">
      <Card title="Named" entries={poll.data.named.entries} bytes={poll.data.named.bytes} fmtBytes={fmtBytes} />
      <Card title="Anonymous" entries={poll.data.anonymous.entries} bytes={poll.data.anonymous.bytes} fmtBytes={fmtBytes} />
    </div>
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
