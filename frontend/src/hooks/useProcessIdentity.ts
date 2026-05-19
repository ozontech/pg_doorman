import { useEffect, useRef } from "react";
import { toast } from "sonner";
import type { OverviewDto } from "../types";

/**
 * Identity-based restart detection. The previous heuristic — "counter
 * total fell, must be a restart" — false-positived on every RELOAD that
 * dropped dynamic pools, because the backend sums query_count_total
 * across the live pool set. pid plus started_at_ms is the source of
 * truth: if either moves between polls, the pooler restarted; if
 * neither moves, no toast even when counters legitimately decrease.
 */

interface ProcessIdentity {
  pid: number;
  started_at_ms: number;
  uptime_seconds: number;
}

// Host-scoped so two pooler tabs do not overwrite each other's last
// known identity.
const KEY = `pgdoorman.proc.identity.${
  typeof window !== "undefined" ? window.location.host : "any"
}`;

function load(): ProcessIdentity | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<ProcessIdentity>;
    if (
      typeof parsed.pid !== "number" ||
      typeof parsed.started_at_ms !== "number" ||
      typeof parsed.uptime_seconds !== "number"
    ) {
      return null;
    }
    return parsed as ProcessIdentity;
  } catch {
    return null;
  }
}

function save(v: ProcessIdentity) {
  try {
    localStorage.setItem(KEY, JSON.stringify(v));
  } catch {
    /* private mode / quota — no-op. */
  }
}

/**
 * Compare the current process identity against the one cached from the
 * previous poll. Returns `true` exactly once per real restart so the
 * caller can toast or annotate. Returns `false` when:
 * - `cur` is the same identity as `prev` (steady state),
 * - `prev` is null (first poll after a fresh tab load — nothing to
 *   compare against; we accept the current identity as the baseline
 *   without notifying).
 *
 * Three independent signals because PID recycle is a real risk on
 * long-running hosts (Linux `kernel.pid_max` defaults to 32 768) and
 * STARTED_AT is captured lazily on first read, so a back-to-back
 * restart can in principle yield identical pid + started_at_ms tuples.
 * `uptime_seconds < prev.uptime_seconds` closes that loophole.
 */
export function isRealRestart(
  prev: ProcessIdentity | null,
  cur: ProcessIdentity,
): boolean {
  if (!prev) return false;
  return (
    prev.pid !== cur.pid ||
    prev.started_at_ms !== cur.started_at_ms ||
    cur.uptime_seconds < prev.uptime_seconds
  );
}

/**
 * React hook variant. Subscribes to an `/api/overview` snapshot and
 * toasts once when identity moves. Persists the identity across page
 * navigations so an operator switching tabs does not re-trigger the
 * toast on the next mount.
 */
export function useProcessIdentityToast(overview: OverviewDto | null) {
  const prevRef = useRef<ProcessIdentity | null>(load());
  useEffect(() => {
    if (!overview) return;
    const cur: ProcessIdentity = {
      pid: overview.pid,
      started_at_ms: overview.started_at_ms,
      uptime_seconds: overview.uptime_seconds,
    };
    if (isRealRestart(prevRef.current, cur)) {
      const prev = prevRef.current!;
      toast.info(
        `pg_doorman restarted (pid ${prev.pid} → ${cur.pid})`,
      );
    }
    prevRef.current = cur;
    save(cur);
  }, [overview]);
}
