import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useValidationErrorState } from "../hooks/useLifecycleEvents";
import type { OverviewDto } from "../types";

/**
 * Persistent banner for lifecycle conditions an operator must not miss:
 *
 * 1. Config validation error — the last deploy step rejected the new
 *    config; the live config is whatever pg_doorman loaded before. The
 *    banner stays up until a successful RELOAD lands.
 * 2. Shutdown — pg_doorman is draining, no new transactions.
 * 3. Migration — binary upgrade in progress, clients moving to the
 *    new process.
 * 4. Unreachable — `/api/overview` has not answered for ~15 s. The UI
 *    is talking to a dead pooler; do not trust the rest of the page.
 *
 * Banners are persistent on purpose: toasts vanish in a few seconds
 * and miss operators who alt-tabbed to a terminal to act on the alert.
 */

const POLL_MS = 5_000;
const STALE_MS = 15_000;

export function LifecycleBanner() {
  const { authHeader } = useAdminAuth();
  // Share the `/api/overview` cache with Sidebar — same queryKey means
  // TanStack Query deduplicates the HTTP call, so two banners-worth of
  // polling do not show up on the wire.
  const { data, dataUpdatedAt, isError } = useQuery({
    queryKey: ["sidebar.overview", authHeader],
    queryFn: ({ signal }) =>
      apiGet<OverviewDto>("/api/overview", authHeader, signal),
    refetchInterval: POLL_MS,
  });
  const validationError = useValidationErrorState();

  // Unreachable: either the last successful fetch is older than the
  // staleness threshold or the most recent attempt errored. Both
  // are operator-visible "the pooler is not talking" signals.
  const lastContactMs = dataUpdatedAt > 0 ? Date.now() - dataUpdatedAt : 0;
  const unreachable =
    (!!data && lastContactMs > STALE_MS) || (isError && !data);
  if (unreachable) {
    return (
      <Bar
        kind="error"
        text={`pg_doorman unreachable — last contact ${formatAgo(lastContactMs)}.`}
      />
    );
  }

  // Validation error sticks until a successful RELOAD lands. The
  // backend rate-limits the event push to 1/sec, so even a SIGHUP loop
  // produces a single visible banner instead of stacking entries.
  if (validationError) {
    return (
      <Bar
        kind="error"
        text={`Config reload rejected: ${validationError.message}`}
      />
    );
  }

  if (data?.shutdown_in_progress) {
    return (
      <Bar
        kind="shutdown"
        text="pg_doorman is draining — new client connections may be refused while open transactions wind down."
      />
    );
  }
  if (data?.migration_in_progress) {
    return (
      <Bar
        kind="migration"
        text="Binary upgrade in progress — clients are migrating to the new process."
      />
    );
  }
  return null;
}

type BarKind = "shutdown" | "migration" | "error";

function Bar({ kind, text }: { kind: BarKind; text: string }) {
  // Amber for shutdown (operator-impacting). Red for error states
  // (validation reject, unreachable). Accent for migration (info).
  const cls =
    kind === "error"
      ? "bg-danger/15 text-danger border-danger/40"
      : kind === "shutdown"
        ? "bg-warning/15 text-warning border-warning/40"
        : "bg-accent/15 text-accent border-accent/40";
  return (
    <div
      role="status"
      className={`w-full border-b px-4 py-2 text-sm font-mono ${cls}`}
    >
      {text}
    </div>
  );
}

function formatAgo(ms: number): string {
  if (ms < 1_000) return "just now";
  if (ms < 60_000) return `${Math.round(ms / 1_000)}s ago`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m ago`;
  return `${Math.round(ms / 3_600_000)}h ago`;
}
