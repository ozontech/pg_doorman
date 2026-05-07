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
import { InfoLabel } from "../components/InfoLabel";
import { MiniSparkline } from "../components/MiniSparkline";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useHistory } from "../hooks/useHistory";
import { usePoll } from "../hooks/usePoll";
import { describeSqlstate } from "../lib/sqlstate";
import { evaluatePool } from "../lib/thresholds";
import { tip } from "../lib/tooltips";
import type {
  PoolCoordinatorDto,
  PoolCoordinatorRowDto,
  PoolDto,
  PoolScalingDto,
  PoolScalingRowDto,
  PoolsDto,
} from "../types";

interface AdminActionResponse {
  ts: number;
  action: string;
  // List of pool ids the action ran against, e.g. ["app_user@app_db"].
  // `affected_pools` retains the count for compatibility.
  affected?: string[];
  affected_pools?: number;
  error?: string;
  // Set on no_matching_db / no_matching_pool 404s.
  db?: string;
  user?: string;
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
  // Coordinator + scaling are admin-only globals; operators following a
  // saturation alert from the Pool detail page need to see "is the
  // database-level cap also pinned" and "is the scaler stuck on
  // anticipation timeouts" without flipping over to the Config tab.
  const coordPoll = usePoll<PoolCoordinatorDto>(
    (signal) => apiGet<PoolCoordinatorDto>("/api/pool_coordinator", authHeader, signal),
    POLL_MS * 2,
  );
  const scalingPoll = usePoll<PoolScalingDto>(
    (signal) => apiGet<PoolScalingDto>("/api/pool_scaling", authHeader, signal),
    POLL_MS * 2,
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
    const saturation = pool.max_connections > 0 ? pool.active / pool.max_connections : 0;
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
  const saturation = pool.max_connections > 0 ? pool.active / pool.max_connections : 0;
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

      {/*
        Two-column grid keeps short KV stacks (Latency, Throughput, Connections,
        TLS) side-by-side so the eye does not have to jump across half the
        viewport between label and value. Wider blocks (Coordinator, Pool
        scaling, Errors-by-SQLSTATE, Threshold reasons) span both columns
        because their contents already render as multi-column tables or lists.
      */}
      <div className="grid flex-1 gap-4 px-6 py-6 md:grid-cols-2">
        <Section title="Latency">
          <KV
            label="query p95 / p99"
            value={`${pool.query_p95_ms} / ${pool.query_p99_ms} ms`}
            tip={tip.queryP95}
          />
          <KV
            label="transactions p95 / p99"
            value={`${pool.transactions_p95_ms} / ${pool.transactions_p99_ms} ms`}
            tip={tip.txP95}
          />
          <KV
            label="wait avg / p95"
            value={`${pool.wait_avg_ms} / ${pool.wait_p95_ms} ms`}
            tip={tip.waitAvg}
          />
          <KV
            label="oldest active query"
            value={`${pool.max_active_age_ms} ms`}
            tip={tip.oldestActive}
          />
        </Section>

        <Section title="Throughput (cumulative)">
          <KV
            label="queries total"
            value={pool.queries_total.toLocaleString()}
            tip={tip.queriesTotal}
          />
          <KV
            label="transactions total"
            value={pool.transactions_total.toLocaleString()}
            tip={tip.txTotal}
          />
          <KV
            label="errors total"
            value={pool.errors_total.toLocaleString()}
            tip={tip.errorsTotal}
          />
        </Section>

        <Section title="Connections">
          <KV
            label="active / idle"
            value={`${pool.active} / ${pool.idle}`}
            tip={tip.connectionsActiveIdle}
          />
          <KV
            label="connections / max"
            value={`${pool.connections} / ${pool.max_connections}`}
            tip={tip.connectionsTotal}
          />
          <KV label="waiting" value={String(pool.waiting)} tip={tip.waiting} />
          <KV label="pool mode" value={pool.pool_mode} tip={tip.poolMode} />
          <KV label="paused" value={pool.paused ? "yes" : "no"} tip={tip.paused} />
          <KV label="epoch" value={String(pool.epoch)} tip={tip.epoch} />
        </Section>

        <Section title="TLS & fallback">
          <KV
            label="fallback active"
            value={pool.fallback_active ? "yes (Patroni cooldown)" : "no"}
            tip={tip.fallbackActive}
          />
          <KV
            label="TLS handshake errors total"
            value={pool.tls_handshake_errors_total.toLocaleString()}
            tip={tip.tlsHandshakeErrors}
          />
          <KV
            label="TLS backend connections"
            value={String(pool.tls_backend_connections)}
            tip={tip.tlsBackendConnections}
          />
        </Section>

        <Section title="Coordinator" wide>
          <CoordinatorBlock
            row={coordPoll.data?.databases.find((d) => d.database === pool.database) ?? null}
          />
        </Section>

        <Section title="Pool scaling" wide>
          <ScalingBlock
            row={
              scalingPoll.data?.pools.find(
                (p) => p.user === pool.user && p.database === pool.database,
              ) ?? null
            }
          />
        </Section>

        <Section title="Errors by SQLSTATE" wide>
          <SqlstateBreakdown errors={pool.errors_by_sqlstate} />
        </Section>

        {evalResult && evalResult.reasons.length > 0 && (
          <Section title="Threshold reasons" wide>
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

// Admin action bar — Pause / Resume / Reconnect target this single
// user@db pool via POST /api/admin/{action}?pool=<user@db>, plus the
// global Reload at the right edge. Pool-scoped is the default since
// pg_doorman 3.7: scoping by ?db= still works for tools that need
// database-wide blast radius, but the UI's most precise click should
// not surprise an operator with cross-tenant impact.
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
          ? `/api/admin/${action}?pool=${encodeURIComponent(pool.id)}`
          : `/api/admin/${action}`;
      const res = await apiPost<AdminActionResponse>(url, authHeader);
      if (res.error) {
        setFeedback({ tone: "err", text: `${action} failed: ${res.error}` });
      } else {
        const ids = res.affected ?? [];
        const label =
          ids.length > 0
            ? `${action} done · ${ids.join(", ")}`
            : `${action} done · 0 pools touched`;
        setFeedback({ tone: "ok", text: label });
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
          title="PAUSE: stop handing out backends on this user@db pool only. In-flight transactions keep running. Use during a deploy or schema migration."
        >
          pause
        </button>
        <button
          type="button"
          disabled={pending !== null || !pool.paused}
          onClick={() => setConfirm({ action: "resume", scope: "pool" })}
          className={buttonClass("default")}
          title="RESUME: undo PAUSE on this user@db pool. Queued clients get backends as soon as they are returned."
        >
          resume
        </button>
        <button
          type="button"
          disabled={pending !== null}
          onClick={() => setConfirm({ action: "reconnect", scope: "pool" })}
          className={buttonClass("warning")}
          title="RECONNECT: drop idle backends on this user@db pool and refuse the active ones when they finish. Use after a Postgres role/grant change so cached connections pick it up."
        >
          reconnect
        </button>
        <button
          type="button"
          disabled={pending !== null}
          onClick={() => setConfirm({ action: "reload", scope: "global" })}
          className={buttonClass("danger")}
          title="RELOAD CONFIG: re-read the TOML file. Most settings apply on the next backend; pool size shrinks via natural drain. Affects every pool — confirm before clicking."
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
          database={pool.database}
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
  database,
  pending,
  onCancel,
  onConfirm,
}: {
  action: string;
  database: string;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [typed, setTyped] = useState("");
  const required = action.toUpperCase();
  const body = (() => {
    switch (action) {
      case "pause":
        return `Pause stops new checkouts on the '${database}' pool of this user only. Existing transactions continue.`;
      case "resume":
        return `Resume re-enables checkouts on the '${database}' pool of this user.`;
      case "reconnect":
        return `Reconnect drops idle backends on the '${database}' pool of this user and refuses the active ones when they return. Use after a role or grant change.`;
      case "reload":
        return "Reload re-reads pg_doorman.toml on every pool. Pool sizes shrink via natural drain.";
      default:
        return "";
    }
  })();
  const title = action === "reload" ? "RELOAD pg_doorman?" : `${required} ${database}?`;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-bg/80 backdrop-blur-sm">
      <div className="w-96 border border-border bg-surface p-6 text-sm">
        <h2 className="mb-3 font-mono text-base font-semibold text-text">{title}</h2>
        <p className="mb-4 text-text-muted">
          {body} Type <span className="font-mono text-text">{required}</span> to confirm.
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

function Section({
  title,
  wide,
  children,
}: {
  title: string;
  // Span both grid columns. Used for blocks whose internal layout is
  // already multi-column (Coordinator, Pool scaling) or whose content is
  // a list (Threshold reasons, SQLSTATE breakdown) where two side-by-side
  // copies would be visually noisy.
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <section className={wide ? "md:col-span-2" : undefined}>
      <h2 className="mb-2 text-xs uppercase tracking-[0.2em] text-text-dim">{title}</h2>
      <div className="border border-border bg-surface p-4">{children}</div>
    </section>
  );
}

function KV({ label, value, tip }: { label: string; value: string; tip?: string }) {
  return (
    <div className="flex items-baseline justify-between border-b border-border/50 py-1 last:border-b-0">
      <InfoLabel tip={tip} className="text-text-muted">
        {label}
      </InfoLabel>
      <span className="font-mono text-text tabular">{value}</span>
    </div>
  );
}

function CoordinatorBlock({ row }: { row: PoolCoordinatorRowDto | null }) {
  if (!row) {
    return (
      <p className="text-sm text-text-dim">
        No coordinator row for this database. The coordinator only tracks
        databases with an active backend connection cap; if you do not see a
        row, max_db_conn is unlimited.
      </p>
    );
  }
  const free = row.max_db_conn > row.current ? row.max_db_conn - row.current : 0;
  const reserve_free = row.reserve_size > row.reserve_used ? row.reserve_size - row.reserve_used : 0;
  return (
    <>
      <KV label="max_db_conn" value={String(row.max_db_conn)} tip={tip.coordMaxDbConn} />
      <KV label="current" value={`${row.current} (${free} free)`} tip={tip.coordCurrent} />
      <KV
        label="reserve"
        value={`${row.reserve_used} / ${row.reserve_size} used (${reserve_free} free)`}
        tip={tip.coordReserveUsed}
      />
      <KV label="evictions" value={row.evictions.toLocaleString()} tip={tip.coordEvictions} />
      <KV
        label="reserve acquisitions"
        value={row.reserve_acq.toLocaleString()}
        tip={tip.coordReserveAcq}
      />
      <KV
        label="exhaustions"
        value={row.exhaustions.toLocaleString()}
        tip={tip.coordExhaustions}
      />
    </>
  );
}

function ScalingBlock({ row }: { row: PoolScalingRowDto | null }) {
  if (!row) {
    return (
      <p className="text-sm text-text-dim">
        No scaling counters for this pool yet. The first checkout will create
        the row.
      </p>
    );
  }
  return (
    <>
      <KV label="in-flight creates" value={String(row.inflight)} tip={tip.scalingInflight} />
      <KV
        label="creates total"
        value={row.creates.toLocaleString()}
        tip={tip.scalingCreates}
      />
      <KV
        label="gate waits"
        value={`${row.gate_waits.toLocaleString()} (${row.gate_budget_ex.toLocaleString()} budget exceeded)`}
        tip={tip.scalingGateWaits}
      />
      <KV
        label="anticipation: notified"
        value={row.antic_notify.toLocaleString()}
        tip={tip.scalingAnticNotify}
      />
      <KV
        label="anticipation: timed out"
        value={row.antic_timeout.toLocaleString()}
        tip={tip.scalingAnticTimeout}
      />
      <KV
        label="create fallback"
        value={row.create_fallback.toLocaleString()}
        tip={tip.scalingCreateFallback}
      />
      <KV
        label="replenish deferred"
        value={row.replenish_def.toLocaleString()}
        tip={tip.scalingReplenishDef}
      />
    </>
  );
}

function SqlstateBreakdown({ errors }: { errors?: Record<string, number> }) {
  const entries = errors ? Object.entries(errors).sort((a, b) => b[1] - a[1]) : [];
  if (entries.length === 0)
    return <p className="text-sm text-text-dim">No errors recorded for this pool yet. The SQLSTATE breakdown fills in the moment a query returns one.</p>;
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

