import { useCallback, useEffect, useRef, useState } from "react";
import { apiGet } from "../api";
import { PageHero } from "../components/PageHero";
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
        <p className="mt-2 text-sm text-danger">
          Could not load logs: {poll.error.message}. Try Sign out → Sign in to refresh credentials, or check whether pg_doorman is running.
        </p>
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
        help={{
          definition:
            "Live pooler log stream via LogTap. Filter by SQLSTATE (e.g. 53300), client (#c123), or module (auth, pool, stats). Pause freezes the view; new lines accumulate in the backend ring buffer. The tap stops 2 minutes after the last reader.",
          source: "LogTap side-channel · SET log_level = '…' to change verbosity",
          related: ["SHOW LOG_LEVEL", "journalctl -u pg_doorman"],
          thresholds: {
            healthy: "drops = 0",
            warn: "drops > 0 — buffer overflow",
            crit: "drops sustained — raise [web].log_tap_max_entries or narrow filter",
          },
          docsHref:
            "https://ozontech.github.io/pg_doorman/observability/json-logging.html",
        }}
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
    return <p className="p-6 text-text-dim">Opening log tap…</p>;
  }
  if (meta.capacity === 0) {
    return (
      <div className="p-6 text-sm leading-relaxed">
        <p className="font-semibold text-warning">Log streaming is turned off in the running config.</p>
        <p className="mt-2 text-text-muted">
          Set <code className="rounded bg-surface px-1.5 py-0.5 text-text">[web].log_tap_max_entries</code>{" "}
          to a positive number (8192 is a good default) and restart pg_doorman. Until then, the daemon ignores tap requests.
        </p>
      </div>
    );
  }
  if (!meta.tap_active) {
    return (
      <div className="p-6 text-sm leading-relaxed">
        <p className="text-text">LogTap is currently off.</p>
        <p className="mt-2 text-text-muted">
          The tap shuts off 2 minutes after the last viewer. It should re-arm on the next poll — if it stays off after a few seconds, reload the page.
        </p>
      </div>
    );
  }
  return (
    <div className="p-6 text-sm leading-relaxed">
      <p className="text-text">
        Tap is open ({meta.used}/{meta.capacity} buffered) and the filter excludes everything in the buffer.
      </p>
      <p className="mt-2 text-text-muted">
        Either the pooler is idle, or your filter is too narrow — clear the search box, pick &lsquo;all levels&rsquo;, and run any query against pg_doorman. Drops since the tap opened: {meta.dropped_total}.
      </p>
    </div>
  );
}
