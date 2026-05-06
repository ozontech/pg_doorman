import { useCallback } from "react";
import { apiGet } from "../api";
import { Collapsible } from "../components/Collapsible";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";

const FAST_MS = 5000;
const SLOW_MS = 15_000;

const SECTIONS: {
  id: string;
  title: string;
  endpoint: string;
  intervalMs: number;
  open?: boolean;
}[] = [
  { id: "config", title: "config", endpoint: "/api/config", intervalMs: SLOW_MS, open: true },
  { id: "log-level", title: "log_level", endpoint: "/api/log_level", intervalMs: FAST_MS },
  { id: "auth-query", title: "auth_query", endpoint: "/api/auth_query", intervalMs: SLOW_MS },
  { id: "databases", title: "databases", endpoint: "/api/databases", intervalMs: SLOW_MS },
  { id: "users", title: "users", endpoint: "/api/users", intervalMs: SLOW_MS },
  { id: "sockets", title: "sockets", endpoint: "/api/sockets", intervalMs: FAST_MS },
  { id: "pool-scaling", title: "pool_scaling", endpoint: "/api/pool_scaling", intervalMs: FAST_MS },
  { id: "pool-coordinator", title: "pool_coordinator", endpoint: "/api/pool_coordinator", intervalMs: FAST_MS },
];

export default function ConfigState() {
  return (
    <section className="flex flex-col">
      <header className="border-b border-border bg-surface px-4 py-3">
        <h1 className="text-lg font-semibold text-text">Config &amp; state</h1>
        <p className="mt-1 text-xs text-text-muted">
          Read-only inspection of the running pooler. Each section polls its
          own endpoint; collapsed sections do not poll until you open them.
        </p>
      </header>
      {SECTIONS.map((s) => (
        <Collapsible key={s.id} id={`config-${s.id}`} title={s.title} defaultOpen={s.open}>
          <SectionBody endpoint={s.endpoint} intervalMs={s.intervalMs} />
        </Collapsible>
      ))}
    </section>
  );
}

function SectionBody({ endpoint, intervalMs }: { endpoint: string; intervalMs: number }) {
  const { authHeader } = useAdminAuth();
  const fetcher = useCallback(
    (signal: AbortSignal) => apiGet<unknown>(endpoint, authHeader, signal),
    [authHeader, endpoint],
  );
  const poll = usePoll<unknown>(fetcher, intervalMs);

  if (poll.error) {
    return <p className="px-4 py-3 text-sm text-danger">{poll.error.message}</p>;
  }
  if (!poll.data) {
    return <p className="px-4 py-3 text-sm text-text-dim">loading…</p>;
  }
  return (
    <pre className="max-h-96 overflow-auto bg-bg px-4 py-3 text-xs text-text font-mono whitespace-pre-wrap">
      {JSON.stringify(poll.data, null, 2)}
    </pre>
  );
}
