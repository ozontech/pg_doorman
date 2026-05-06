// Full-route pool detail. The right-side drawer that lived inside `Pools.tsx`
// was 28rem wide and forced a 7-block vertical scroll for sparklines, KV
// pairs, threshold reasons, and the SQLSTATE breakdown. Operators flagged it
// as unusable; the comparable patterns in Datadog/Stripe/Lens/GitHub all use
// a full route, so this page takes that shape: identity bar, KPI tile strip,
// tabbed body. The route is `/pools/:poolId` and reuses `/api/pools` —
// nothing on the backend had to move for this view to exist.

import type { ReactNode } from "react";
import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { apiGet, apiPost } from "../api";
import { MiniSparkline } from "../components/MiniSparkline";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useHistory } from "../hooks/useHistory";
import { usePoll } from "../hooks/usePoll";
import { describeSqlstate } from "../lib/sqlstate";
import { evaluatePool } from "../lib/thresholds";
import type { PoolDto, PoolsDto } from "../types";

interface AdminActionResponse {
  ts: number;
  action: string;
  affected_pools?: number;
  error?: string;
}

const POLL_MS = 1500;
const HISTORY_KEY_PREFIX = "pools.detail";

interface RowSnap {
  ts: number;
  saturation: number;
  query_p95_ms: number;
  errors_total: number;
  queries_total: number;
  waiting: number;
  qps: number;
  errors_per_s: number;
}

export default function PoolDetail() {
  const { poolId: rawId } = useParams<{ poolId: string }>();
  const poolId = decodeURIComponent(rawId ?? "");
  const { authHeader } = useAdminAuth();

  const poll = usePoll<PoolsDto>(
    (signal) => apiGet<PoolsDto>("/api/pools", authHeader, signal),
    POLL_MS,
  );
  const history = useHistory<RowSnap>(`${HISTORY_KEY_PREFIX}.${poolId}`, 240);

  const pool: PoolDto | undefined = useMemo(
    () => poll.data?.pools.find((p) => p.id === poolId),
    [poll.data, poolId],
  );

  // Push history on each successful poll. Computes per-second rates from the
  // previous snapshot so the tile strip can show a live qps and errors/s
  // without the operator having to math the cumulative counters.
  const evalResult = useMemo(() => (pool ? evaluatePool(pool, undefined) : null), [pool]);

  if (!pool && poll.error) {
    return (
      <section className="px-6 py-8">
        <BackLink />
        <p className="mt-4 text-sm text-danger">{poll.error.message}</p>
      </section>
    );
  }
  if (!pool) {
    return (
      <section className="px-6 py-8">
        <BackLink />
        <p className="mt-4 text-sm text-text-dim">Loading pool {poolId}…</p>
      </section>
    );
  }

  // Push history snapshot (single line — kept inside the render path because
  // we only have data once `pool` is non-null and the cadence is set by
  // `usePoll`, so this is safe and matches the pattern used in Overview.tsx).
  const last = history.history[history.history.length - 1];
  if (!last || last.ts !== poll.data!.ts) {
    const saturation = pool.max_connections > 0 ? pool.connections / pool.max_connections : 0;
    let qps = 0;
    let eps = 0;
    if (last) {
      const dt = (poll.data!.ts - last.ts) / 1000;
      if (dt > 0) {
        qps = Math.max(0, (pool.queries_total - last.queries_total) / dt);
        eps = Math.max(0, (pool.errors_total - last.errors_total) / dt);
      }
    }
    history.push({
      ts: poll.data!.ts,
      saturation,
      query_p95_ms: pool.query_p95_ms,
      errors_total: pool.errors_total,
      queries_total: pool.queries_total,
      waiting: pool.waiting,
      qps,
      errors_per_s: eps,
    });
  }

  const series = (extract: (s: RowSnap) => number) => history.history.map(extract);
  const saturation = pool.max_connections > 0 ? pool.connections / pool.max_connections : 0;
  const latestQps = history.history[history.history.length - 1]?.qps ?? 0;
  const latestEps = history.history[history.history.length - 1]?.errors_per_s ?? 0;

  return (
    <section className="flex min-h-screen flex-col">
      <header className="border-b border-border bg-surface px-6 py-4">
        <div className="flex flex-wrap items-baseline justify-between gap-4">
          <div>
            <BackLink />
            <h1 className="mt-2 font-mono text-xl font-semibold text-text">{pool.id}</h1>
            <p className="mt-1 text-xs text-text-muted">
              {pool.user} → {pool.database} on {pool.host}:{pool.port} · mode {pool.pool_mode}
              {pool.paused && (
                <span className="ml-2 inline-block bg-warning/20 px-2 py-0.5 text-warning">
                  PAUSED
                </span>
              )}
              {evalResult?.severity === "critical" && (
                <span className="ml-2 inline-block bg-danger/20 px-2 py-0.5 text-danger">
                  CRITICAL
                </span>
              )}
              {evalResult?.severity === "degraded" && (
                <span className="ml-2 inline-block bg-warning/20 px-2 py-0.5 text-warning">
                  DEGRADED
                </span>
              )}
            </p>
          </div>
          <PoolActions pool={pool} />
        </div>
      </header>

      <div className="grid grid-cols-2 gap-3 border-b border-border bg-surface px-6 py-4 md:grid-cols-3 lg:grid-cols-6">
        <Tile
          label="saturation"
          value={`${(saturation * 100).toFixed(0)}%`}
          tone={saturation >= 0.9 ? "danger" : saturation >= 0.7 ? "warning" : "ok"}
          spark={series((s) => s.saturation * 100)}
          sparkMin={0}
          sparkMax={100}
        />
        <Tile
          label="query p95"
          value={`${pool.query_p95_ms} ms`}
          tone={pool.query_p95_ms > 500 ? "danger" : pool.query_p95_ms > 100 ? "warning" : "ok"}
          spark={series((s) => s.query_p95_ms)}
        />
        <Tile
          label="waiting"
          value={String(pool.waiting)}
          tone={pool.waiting > 0 ? "warning" : "ok"}
          spark={series((s) => s.waiting)}
        />
        <Tile
          label="errors / s"
          value={latestEps.toFixed(2)}
          tone={latestEps > 1 ? "danger" : latestEps > 0.1 ? "warning" : "ok"}
          spark={series((s) => s.errors_per_s)}
        />
        <Tile
          label="oldest active"
          value={`${pool.max_active_age_ms} ms`}
          tone={
            pool.max_active_age_ms > 300_000
              ? "danger"
              : pool.max_active_age_ms > 30_000
                ? "warning"
                : "ok"
          }
          spark={[]}
        />
        <Tile label="qps" value={latestQps.toFixed(1)} tone="ok" spark={series((s) => s.qps)} />
      </div>

      <div className="flex-1 px-6 py-6">
        <Section title="Latency">
          <KV label="query p95 / p99" value={`${pool.query_p95_ms} / ${pool.query_p99_ms} ms`} />
          <KV
            label="transactions p95 / p99"
            value={`${pool.transactions_p95_ms} / ${pool.transactions_p99_ms} ms`}
          />
          <KV label="wait avg / p95" value={`${pool.wait_avg_ms} / ${pool.wait_p95_ms} ms`} />
          <KV label="oldest active query" value={`${pool.max_active_age_ms} ms`} />
        </Section>

        <Section title="Throughput (cumulative)">
          <KV label="queries total" value={pool.queries_total.toLocaleString()} />
          <KV label="transactions total" value={pool.transactions_total.toLocaleString()} />
          <KV label="errors total" value={pool.errors_total.toLocaleString()} />
        </Section>

        <Section title="Connections">
          <KV label="active / idle" value={`${pool.active} / ${pool.idle}`} />
          <KV label="connections / max" value={`${pool.connections} / ${pool.max_connections}`} />
          <KV label="waiting" value={String(pool.waiting)} />
          <KV label="pool mode" value={pool.pool_mode} />
          <KV label="paused" value={pool.paused ? "yes" : "no"} />
          <KV label="epoch" value={String(pool.epoch)} />
        </Section>

        <Section title="TLS & fallback">
          <KV
            label="fallback active"
            value={pool.fallback_active ? "yes (Patroni cooldown)" : "no"}
          />
          <KV
            label="TLS handshake errors total"
            value={pool.tls_handshake_errors_total.toLocaleString()}
          />
          <KV label="TLS backend connections" value={String(pool.tls_backend_connections)} />
        </Section>

        <Section title="Errors by SQLSTATE">
          <SqlstateBreakdown errors={pool.errors_by_sqlstate} />
        </Section>

        {evalResult && evalResult.reasons.length > 0 && (
          <Section title="Threshold reasons">
            <ul className="space-y-1 text-text-muted">
              {evalResult.reasons.map((r) => (
                <li key={r}>· {r}</li>
              ))}
            </ul>
          </Section>
        )}
      </div>
    </section>
  );
}

// Admin action bar — Pause / Resume / Reconnect against this pool's
// database via POST /api/admin/{action}?db=<database>, plus the global
// Reload at the right edge. Each click opens a confirmation modal: write
// actions on a live pool are easy to mis-click and the modal makes the
// operator type "yes" before the network call goes out.
function PoolActions({ pool }: { pool: PoolDto }) {
  const { authHeader } = useAdminAuth();
  const [pending, setPending] = useState<null | string>(null);
  const [confirm, setConfirm] = useState<null | { action: string; scope: "pool" | "global" }>(null);
  const [feedback, setFeedback] = useState<null | { tone: "ok" | "err"; text: string }>(null);

  const trigger = async (action: string, scope: "pool" | "global") => {
    setPending(action);
    setFeedback(null);
    try {
      const url =
        scope === "pool"
          ? `/api/admin/${action}?db=${encodeURIComponent(pool.database)}`
          : `/api/admin/${action}`;
      const res = await apiPost<AdminActionResponse>(url, authHeader);
      if (res.error) {
        setFeedback({ tone: "err", text: `${action} failed: ${res.error}` });
      } else {
        setFeedback({
          tone: "ok",
          text: `${action} OK · ${res.affected_pools ?? 0} pool(s) affected`,
        });
      }
    } catch (e) {
      setFeedback({ tone: "err", text: `${action} failed: ${e instanceof Error ? e.message : String(e)}` });
    } finally {
      setPending(null);
      setConfirm(null);
    }
  };

  const buttonClass = (variant: "default" | "danger" | "warning") => {
    const base =
      "border px-3 py-1 text-xs font-mono uppercase tracking-wider transition-colors disabled:opacity-50";
    if (variant === "danger") return `${base} border-danger text-danger hover:bg-danger/10`;
    if (variant === "warning") return `${base} border-warning text-warning hover:bg-warning/10`;
    return `${base} border-border-strong text-text-muted hover:text-accent`;
  };

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={pending !== null || pool.paused}
          onClick={() => setConfirm({ action: "pause", scope: "pool" })}
          className={buttonClass("warning")}
          title="Block new client checkouts on this pool. Active transactions continue."
        >
          pause
        </button>
        <button
          type="button"
          disabled={pending !== null || !pool.paused}
          onClick={() => setConfirm({ action: "resume", scope: "pool" })}
          className={buttonClass("default")}
          title="Re-enable client checkouts after a pause."
        >
          resume
        </button>
        <button
          type="button"
          disabled={pending !== null}
          onClick={() => setConfirm({ action: "reconnect", scope: "pool" })}
          className={buttonClass("warning")}
          title="Bump pool epoch and drain idle connections. Active connections refused on return."
        >
          reconnect
        </button>
        <button
          type="button"
          disabled={pending !== null}
          onClick={() => setConfirm({ action: "reload", scope: "global" })}
          className={buttonClass("danger")}
          title="Reload the entire pg_doorman config from disk. Affects every pool."
        >
          reload (global)
        </button>
      </div>
      {feedback && (
        <div
          className={`text-xs font-mono ${
            feedback.tone === "ok" ? "text-success" : "text-danger"
          }`}
        >
          {feedback.text}
        </div>
      )}
      {confirm && (
        <ConfirmModal
          action={confirm.action}
          scopeLabel={confirm.scope === "pool" ? `database "${pool.database}"` : "the entire pooler"}
          pending={pending !== null}
          onCancel={() => setConfirm(null)}
          onConfirm={() => trigger(confirm.action, confirm.scope)}
        />
      )}
    </div>
  );
}

function ConfirmModal({
  action,
  scopeLabel,
  pending,
  onCancel,
  onConfirm,
}: {
  action: string;
  scopeLabel: string;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [typed, setTyped] = useState("");
  const required = action.toUpperCase();
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-bg/80 backdrop-blur-sm">
      <div className="w-96 border border-border bg-surface p-6 text-sm">
        <h2 className="mb-3 font-mono text-base font-semibold text-text">
          {action.toUpperCase()} {scopeLabel}?
        </h2>
        <p className="mb-4 text-text-muted">
          This is a write action. Type <span className="font-mono text-text">{required}</span> below to confirm.
        </p>
        <input
          autoFocus
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          className="mb-4 w-full border border-border-strong bg-surface-2 px-2 py-1 font-mono text-sm text-text"
          placeholder={required}
        />
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            disabled={pending}
            className="border border-border-strong px-3 py-1 text-xs font-mono uppercase tracking-wider text-text-muted hover:text-text"
          >
            cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={pending || typed !== required}
            className="border border-danger bg-danger/10 px-3 py-1 text-xs font-mono uppercase tracking-wider text-danger hover:bg-danger/20 disabled:opacity-50"
          >
            {pending ? "running…" : `run ${action}`}
          </button>
        </div>
      </div>
    </div>
  );
}

function BackLink() {
  return (
    <Link
      to="/pools"
      className="text-xs uppercase tracking-wide text-text-muted hover:text-accent"
    >
      ← all pools
    </Link>
  );
}

function Tile({
  label,
  value,
  tone,
  spark,
  sparkMin,
  sparkMax,
}: {
  label: string;
  value: string;
  tone: "ok" | "warning" | "danger";
  spark: number[];
  sparkMin?: number;
  sparkMax?: number;
}) {
  const stroke =
    tone === "danger" ? "rgb(255 77 77)" : tone === "warning" ? "rgb(255 176 0)" : "rgb(57 211 83)";
  const valueClass =
    tone === "danger" ? "text-danger" : tone === "warning" ? "text-warning" : "text-text";
  return (
    <div className="border border-border bg-surface px-3 py-2">
      <div className="text-[10px] uppercase tracking-[0.2em] text-text-dim">{label}</div>
      <div className={`mt-1 font-mono text-lg font-semibold tabular ${valueClass}`}>{value}</div>
      {spark.length > 0 && (
        <div className="mt-1">
          <MiniSparkline
            values={spark}
            stroke={stroke}
            width={140}
            height={20}
            min={sparkMin}
            max={sparkMax}
          />
        </div>
      )}
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-8">
      <h2 className="mb-2 text-xs uppercase tracking-[0.2em] text-text-dim">{title}</h2>
      <div className="border border-border bg-surface p-4">{children}</div>
    </section>
  );
}

function KV({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between border-b border-border/50 py-1 last:border-b-0">
      <span className="text-text-muted">{label}</span>
      <span className="font-mono text-text tabular">{value}</span>
    </div>
  );
}

function SqlstateBreakdown({ errors }: { errors?: Record<string, number> }) {
  const entries = errors ? Object.entries(errors).sort((a, b) => b[1] - a[1]) : [];
  if (entries.length === 0)
    return <p className="text-sm text-text-dim">No errors classified yet.</p>;
  return (
    <ul className="space-y-1 tabular">
      {entries.map(([code, count]) => (
        <li
          key={code}
          className="flex items-baseline justify-between border-b border-border/50 py-1 last:border-b-0"
        >
          <span className="flex flex-col">
            <span className="font-mono text-text">{code}</span>
            <span className="text-xs text-text-dim">{describeSqlstate(code)}</span>
          </span>
          <span className="font-mono text-text">{count}</span>
        </li>
      ))}
    </ul>
  );
}

