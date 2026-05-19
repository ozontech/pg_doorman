import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import type { OverviewDto } from "../types";

/**
 * Persistent banner shown while the pooler is in a transient lifecycle
 * state — draining for shutdown or migrating clients to a new binary.
 * Toasts disappear after a few seconds and are easy to miss; a banner
 * stays visible until the underlying flag clears, which is what an
 * operator watching the dashboard needs.
 *
 * Polls `/api/overview` independently of the Sidebar so the banner
 * renders even on pages whose own data source ignores the lifecycle
 * fields.
 */

const POLL_MS = 5_000;

export function LifecycleBanner() {
  const { authHeader } = useAdminAuth();
  const { data } = useQuery({
    queryKey: ["lifecycle-banner.overview", authHeader],
    queryFn: ({ signal }) =>
      apiGet<OverviewDto>("/api/overview", authHeader, signal),
    refetchInterval: POLL_MS,
  });

  if (!data) return null;
  if (data.shutdown_in_progress) {
    return (
      <Bar
        kind="shutdown"
        text="pg_doorman is draining — new client connections may be refused while open transactions wind down."
      />
    );
  }
  if (data.migration_in_progress) {
    return (
      <Bar
        kind="migration"
        text="Binary upgrade in progress — clients are migrating to the new process."
      />
    );
  }
  return null;
}

function Bar({ kind, text }: { kind: "shutdown" | "migration"; text: string }) {
  // Amber for shutdown (operator-impacting), cyan for migration (informational).
  const cls =
    kind === "shutdown"
      ? "bg-warning/15 text-warning-strong border-warning/40"
      : "bg-accent/15 text-accent-strong border-accent/40";
  return (
    <div
      role="status"
      className={`w-full border-b px-4 py-2 text-sm font-mono ${cls}`}
    >
      {text}
    </div>
  );
}
