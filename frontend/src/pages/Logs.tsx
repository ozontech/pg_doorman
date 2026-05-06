import { useCallback, useEffect, useRef, useState } from "react";
import { apiGet } from "../api";
import { PageHero } from "../components/PageHero";
import { SectionHeader } from "../components/SectionHeader";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type { LogEntryDto, LogsDto } from "../types";

const POLL_MS = 1500;
const MAX_KEEP = 500;
const LEVEL_OPTIONS = ["", "ERROR", "WARN", "INFO", "DEBUG", "TRACE"];

const LEVEL_COLOR: Record<string, string> = {
  ERROR: "text-danger",
  WARN: "text-warning",
  INFO: "text-text",
  DEBUG: "text-text-muted",
  TRACE: "text-text-dim",
};

export default function Logs() {
  const { authHeader } = useAdminAuth();
  const [level, setLevel] = useState("");
  const [target, setTarget] = useState("");
  const [autoScroll, setAutoScroll] = useState(true);
  const [paused, setPaused] = useState(false);
  const [lines, setLines] = useState<LogEntryDto[]>([]);
  const [meta, setMeta] = useState<{
    tap_active: boolean;
    used: number;
    capacity: number;
    dropped_before: number;
    dropped_total: number;
  } | null>(null);
  const sinceRef = useRef(0);
  const tailRef = useRef<HTMLDivElement | null>(null);

  // Reset stream when level changes (full refetch from seq 0). The text
  // filter stays client-side so changing it doesn't drop the buffer.
  useEffect(() => {
    sinceRef.current = 0;
    setLines([]);
  }, [level]);

  const fetcher = useCallback(
    async (signal: AbortSignal): Promise<LogsDto> => {
      const params = new URLSearchParams();
      params.set("since", String(sinceRef.current));
      params.set("max", "200");
      if (level) params.set("level", level);
      return apiGet<LogsDto>(`/api/logs?${params.toString()}`, authHeader, signal);
    },
    [authHeader, level],
  );

  const poll = usePoll<LogsDto>(fetcher, paused ? 60_000 : POLL_MS);

  // Append new entries on each successful poll.
  useEffect(() => {
    if (!poll.data) return;
    setMeta({
      tap_active: poll.data.tap_active,
      used: poll.data.tap_used_entries,
      capacity: poll.data.tap_capacity_entries,
      dropped_before: poll.data.dropped_before,
      dropped_total: poll.data.dropped_total,
    });
    if (poll.data.entries.length > 0) {
      sinceRef.current = poll.data.next_seq;
      setLines((prev) => {
        const next = [...prev, ...poll.data!.entries];
        if (next.length > MAX_KEEP) return next.slice(next.length - MAX_KEEP);
        return next;
      });
    } else if (poll.data.next_seq > sinceRef.current) {
      // server moved forward without entries (e.g. filter rejected all).
      sinceRef.current = poll.data.next_seq;
    }
  }, [poll.data]);

  useEffect(() => {
    if (autoScroll && tailRef.current) {
      tailRef.current.scrollIntoView({ behavior: "auto" });
    }
  }, [lines, autoScroll]);

  if (poll.error) {
    return (
      <section className="p-6">
        <h1 className="text-lg font-semibold text-text">Logs</h1>
        <p className="mt-2 text-sm text-danger">{poll.error.message}</p>
      </section>
    );
  }

  const fmtTs = (ms: number) => {
    const d = new Date(ms);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, "0")}`;
  };

  return (
    <section className="flex h-screen flex-col">
      <PageHero
        title="Logs"
        description="Live pooler log without ssh-ing onto the host. Filter by level or by any text — module, client id (#c123), SQLSTATE, message fragment. The tap turns itself off 30 s after you leave the page, so it costs nothing when nobody is reading."
      />
      <SectionHeader
        title="Stream"
        what="Newest entries appended below; last 500 lines kept in memory."
        how={"Poll cadence 1.5 s. Pause holds the buffer and slows polling to 60 s."}
        normal="drops > 0 = consumer fell behind, raise log_tap_max_entries or filter narrower."
      />
      <div className="flex items-center gap-3 border-b border-border px-6 py-3">
        <select
          value={level}
          onChange={(e) => setLevel(e.target.value)}
          className="rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        >
          {LEVEL_OPTIONS.map((l) => (
            <option key={l} value={l}>
              {l === "" ? "all levels" : `${l}+`}
            </option>
          ))}
        </select>
        <input
          placeholder="search any text (target or message, e.g. #c235, pool, 53300)"
          title={
            "Case-sensitive substring search across both target and message columns. " +
            "Filter is client-side so the buffer is preserved on each keystroke. " +
            "Use 'pool', 'auth', 'stats' for module subsets; '#c123' for a specific client; " +
            "a SQLSTATE like '53300' or part of the message body."
          }
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          className="w-96 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text font-mono"
        />
        <button
          type="button"
          onClick={() => setPaused((p) => !p)}
          className={`rounded border px-3 py-1 text-sm ${paused ? "border-warning text-warning" : "border-border-strong text-text-muted hover:text-text"}`}
        >
          {paused ? "▶ resume" : "❚❚ pause"}
        </button>
        <label className="flex items-center gap-1 text-xs text-text-muted">
          <input type="checkbox" checked={autoScroll} onChange={(e) => setAutoScroll(e.target.checked)} />
          auto-scroll
        </label>
        {meta && (
          <span className="ml-auto text-xs text-text-dim tabular">
            {meta.tap_active ? "tap on" : "tap off"} · {meta.used}/{meta.capacity} buffered ·
            {" "}drops {meta.dropped_total}
            {meta.dropped_before > 0 && ` · ${meta.dropped_before} lost before since`}
          </span>
        )}
      </div>
      <div className="flex-1 overflow-auto bg-bg font-mono text-xs">
        {(() => {
          const filtered = target
            ? lines.filter(
                (e) => e.target.includes(target) || e.message.includes(target),
              )
            : lines;
          if (filtered.length === 0) return <LogsEmpty meta={meta} />;
          return (
          <table className="w-full">
            <tbody>
              {filtered.map((e) => (
                <tr key={e.seq} className="border-b border-border/40 hover:bg-surface-2">
                  <td className="whitespace-nowrap px-2 py-0.5 text-text-dim tabular">
                    {fmtTs(e.ts_ms)}
                  </td>
                  <td className={`px-2 py-0.5 ${LEVEL_COLOR[e.level] ?? ""}`}>{e.level}</td>
                  <td className="whitespace-nowrap px-2 py-0.5 text-accent">{e.target}</td>
                  <td className="px-2 py-0.5 text-text">{e.message}</td>
                </tr>
              ))}
            </tbody>
          </table>
          );
        })()}
        <div ref={tailRef} />
      </div>
    </section>
  );
}

function LogsEmpty({
  meta,
}: {
  meta: {
    tap_active: boolean;
    used: number;
    capacity: number;
    dropped_before: number;
    dropped_total: number;
  } | null;
}) {
  if (!meta) {
    return <p className="p-6 text-text-dim">connecting to LogTap…</p>;
  }
  if (meta.capacity === 0) {
    return (
      <div className="p-6 text-sm leading-relaxed">
        <p className="font-semibold text-warning">LogTap is disabled in this build.</p>
        <p className="mt-2 text-text-muted">
          Set <code className="rounded bg-surface px-1.5 py-0.5 text-text">[web].log_tap_max_entries</code>{" "}
          to a positive integer (default 8192) and restart pg_doorman to enable streaming logs.
        </p>
      </div>
    );
  }
  if (!meta.tap_active) {
    return (
      <div className="p-6 text-sm leading-relaxed">
        <p className="text-text">LogTap is currently off.</p>
        <p className="mt-2 text-text-muted">
          The tap deactivates 30 s after the last poll; it should turn back on within the next tick. If it stays off, reload the page.
        </p>
      </div>
    );
  }
  return (
    <div className="p-6 text-sm leading-relaxed">
      <p className="text-text">
        Tap is on — buffer holds {meta.used} of {meta.capacity} entries — and nothing matches the current filter yet.
      </p>
      <p className="mt-2 text-text-muted">
        Likely the pooler is idle. Run a query against pg_doorman or widen the level / clear the target filter to see entries.{" "}
        Drops since the tap activated: {meta.dropped_total}.
      </p>
    </div>
  );
}
