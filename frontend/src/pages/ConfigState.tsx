import { useCallback, useMemo, useState, type ReactNode } from "react";
import { apiGet } from "../api";
import { Collapsible } from "../components/Collapsible";
import { InfoLabel } from "../components/InfoLabel";
import { PageHero } from "../components/PageHero";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { usePoll } from "../hooks/usePoll";
import type {
  AuthQueryDto,
  ConfigDto,
  DatabasesDto,
  LogLevelDto,
  PoolCoordinatorDto,
  PoolScalingDto,
  PoolsDto,
  SocketsDto,
  UsersDto,
} from "../types";

const FAST_MS = 5000;
const SLOW_MS = 15_000;

export default function ConfigState() {
  return (
    <section className="flex flex-col">
      <PageHero
        title="Config & state"
        help={{
          definition:
            "Read-only snapshot of what pg_doorman is running with right now. Same content as SHOW CONFIG / SHOW DATABASES / SHOW USERS / SHOW POOL_COORDINATOR / SHOW POOL_SCALING — just queryable from the browser. Compare default vs current after a SIGHUP / RELOAD to confirm the edit took effect.",
          source:
            "SHOW CONFIG · SHOW DATABASES · SHOW USERS · SHOW AUTH_QUERY · SHOW LOG_LEVEL · SHOW STARTUP_PARAMETERS · SHOW SOCKETS · SHOW POOL_SCALING · SHOW POOL_COORDINATOR",
          related: ["RELOAD", "SIGHUP"],
          docsHref:
            "https://ozontech.github.io/pg_doorman/observability/admin-commands.html",
        }}
      />
      <Collapsible id="config-config" title="config" defaultOpen>
        <ConfigPanel />
      </Collapsible>
      <Collapsible id="config-log-level" title="log_level">
        <LogLevelPanel />
      </Collapsible>
      <Collapsible id="config-auth-query" title="auth_query">
        <AuthQueryPanel />
      </Collapsible>
      <Collapsible id="config-databases" title="databases">
        <DatabasesPanel />
      </Collapsible>
      <Collapsible id="config-users" title="users">
        <UsersPanel />
      </Collapsible>
      <Collapsible id="config-startup-parameters" title="startup parameters">
        <StartupParametersPanel />
      </Collapsible>
      <Collapsible id="config-sockets" title="sockets">
        <SocketsPanel />
      </Collapsible>
      <Collapsible id="config-pool-scaling" title="pool_scaling" defaultOpen>
        <PoolScalingPanel />
      </Collapsible>
      <Collapsible id="config-pool-coordinator" title="pool_coordinator">
        <PoolCoordinatorPanel />
      </Collapsible>
    </section>
  );
}

function useEndpoint<T>(endpoint: string, intervalMs: number) {
  const { authHeader } = useAdminAuth();
  const fetcher = useCallback(
    (signal: AbortSignal) => apiGet<T>(endpoint, authHeader, signal),
    [authHeader, endpoint],
  );
  return usePoll<T>(fetcher, intervalMs);
}

function PanelShell({
  loading,
  error,
  children,
}: {
  loading: boolean;
  error: Error | null;
  children: ReactNode;
}) {
  if (error) return <p className="px-6 py-4 text-sm text-danger">{error.message}</p>;
  if (loading) return <p className="px-6 py-4 text-sm text-text-dim">loading…</p>;
  return <>{children}</>;
}

function ConfigPanel() {
  const poll = useEndpoint<ConfigDto>("/api/config", SLOW_MS);
  const [filter, setFilter] = useState("");
  const filtered = useMemo(() => {
    if (!poll.data) return [];
    const q = filter.trim().toLowerCase();
    if (!q) return poll.data.config;
    return poll.data.config.filter(
      (e) => e.key.toLowerCase().includes(q) || e.value.toLowerCase().includes(q),
    );
  }, [poll.data, filter]);

  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      <div className="flex items-center gap-3 px-6 py-3">
        <input
          placeholder="filter key or value…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="w-72 rounded border border-border-strong bg-surface-2 px-2 py-1 text-sm text-text"
        />
        <span className="text-xs text-text-dim tabular">
          {filtered.length} of {poll.data?.config.length ?? 0} keys
        </span>
      </div>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-6 py-2 text-left">Key</th>
            <th className="px-3 py-2 text-left">Default</th>
            <th className="px-3 py-2 text-left">Current</th>
            <th className="px-3 py-2 text-left">Reload-able</th>
          </tr>
        </thead>
        <tbody>
          {filtered.map((e) => {
            const changed = e.default !== "-" && e.default !== e.value;
            return (
              <tr key={e.key} className="border-b border-border/40 hover:bg-surface-2">
                <td className="px-6 py-1.5 font-mono text-xs">
                  <InfoLabel tip={e.doc || undefined}>{e.key}</InfoLabel>
                </td>
                <ConfigValueCell value={e.default} className="text-text-dim" />
                <ConfigValueCell
                  value={e.value}
                  className={changed ? "text-accent" : "text-text-muted"}
                  baseTip={
                    changed
                      ? "Operator-overridden value (differs from built-in default)."
                      : undefined
                  }
                />
                <td className="px-3 py-1.5 text-xs">
                  <span className={e.changeable === "yes" ? "text-success" : "text-text-dim"}>
                    {e.changeable === "yes" ? "yes" : "restart"}
                  </span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </PanelShell>
  );
}

/// Truncates long config values so a single outlier (e.g. `talos.keys` with
/// a comma-joined list of `.pem` paths) cannot blow the table column past
/// the viewport. The full value stays one hover away in the styled tooltip.
const VALUE_TRUNCATE_AT = 64;

function ConfigValueCell({
  value,
  className,
  baseTip,
}: {
  value: string;
  className: string;
  baseTip?: string;
}) {
  const truncated = value.length > VALUE_TRUNCATE_AT;
  const display = truncated ? `${value.slice(0, VALUE_TRUNCATE_AT)}…` : value;
  const tip = truncated ? value : baseTip;
  return (
    <td className={`px-3 py-1.5 font-mono text-xs ${className}`}>
      {tip ? <InfoLabel tip={tip}>{display}</InfoLabel> : display}
    </td>
  );
}

function LogLevelPanel() {
  const poll = useEndpoint<LogLevelDto>("/api/log_level", FAST_MS);
  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      <div className="px-6 py-4">
        <div className="text-xs uppercase tracking-wide text-text-dim">Active filter</div>
        <div className="mt-1 font-mono text-base text-text">{poll.data?.log_level}</div>
        <p className="mt-3 max-w-2xl text-xs text-text-muted">
          RUST_LOG-style filter. Change at runtime with{" "}
          <code className="rounded bg-surface px-1.5 py-0.5 font-mono">{`SET log_level = '…'`}</code>{" "}
          on the admin protocol;{" "}
          <code className="rounded bg-surface px-1.5 py-0.5 font-mono">{`'default'`}</code>{" "}
          resets to the startup level.
        </p>
      </div>
    </PanelShell>
  );
}

function AuthQueryPanel() {
  const poll = useEndpoint<AuthQueryDto>("/api/auth_query", SLOW_MS);
  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      {poll.data?.pools.length === 0 ? (
        <p className="px-6 py-4 text-sm text-text-dim">No pool uses auth_query in this config. Set <code>auth_query = &lsquo;...&rsquo;</code> under [databases.X] to enable per-database password lookups.</p>
      ) : (
        <table className="w-full text-sm tabular">
          <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
            <tr>
              <th className="px-6 py-2 text-left">Database</th>
              <th className="px-3 py-2 text-right">Entries</th>
              <th className="px-3 py-2 text-right">Hit rate</th>
              <th className="px-3 py-2 text-right">Auth ok</th>
              <th className="px-3 py-2 text-right">Auth fail</th>
              <th className="px-3 py-2 text-right">Exec err</th>
              <th className="px-3 py-2 text-right">Dyn pools</th>
            </tr>
          </thead>
          <tbody>
            {poll.data?.pools.map((row) => {
              const total = row.cache_hits + row.cache_misses;
              const hr = total > 0 ? row.cache_hits / total : null;
              const failRate =
                row.auth_success + row.auth_failure > 0
                  ? row.auth_failure / (row.auth_success + row.auth_failure)
                  : 0;
              return (
                <tr key={row.database} className="border-b border-border/40 hover:bg-surface-2">
                  <td className="px-6 py-1.5 font-mono text-xs">{row.database}</td>
                  <td className="px-3 py-1.5 text-right">{row.cache_entries}</td>
                  <td
                    className={`px-3 py-1.5 text-right ${
                      hr !== null && hr < 0.8
                        ? "text-danger"
                        : hr !== null && hr < 0.95
                          ? "text-warning"
                          : ""
                    }`}
                  >
                    {hr === null ? "—" : `${(hr * 100).toFixed(1)}%`}
                  </td>
                  <td className="px-3 py-1.5 text-right">{row.auth_success}</td>
                  <td
                    className={`px-3 py-1.5 text-right ${
                      failRate > 0.05 ? "text-danger" : failRate > 0.005 ? "text-warning" : ""
                    }`}
                  >
                    {row.auth_failure}
                  </td>
                  <td
                    className={`px-3 py-1.5 text-right ${
                      row.executor_errors > 0 ? "text-warning" : ""
                    }`}
                  >
                    {row.executor_errors}
                  </td>
                  <td className="px-3 py-1.5 text-right">{row.dynamic_pools_current}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </PanelShell>
  );
}

function DatabasesPanel() {
  const poll = useEndpoint<DatabasesDto>("/api/databases", SLOW_MS);
  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-6 py-2 text-left">Pool</th>
            <th className="px-3 py-2 text-left">Host:Port</th>
            <th className="px-3 py-2 text-left">Force user</th>
            <th className="px-3 py-2 text-left">Mode</th>
            <th className="px-3 py-2 text-right">Pool size</th>
            <th className="px-3 py-2 text-right">Min</th>
            <th className="px-3 py-2 text-right">Connections</th>
          </tr>
        </thead>
        <tbody>
          {poll.data?.databases.map((d) => (
            <tr key={d.name} className="border-b border-border/40 hover:bg-surface-2">
              <td className="px-6 py-1.5 font-mono text-xs">{d.name}</td>
              <td className="px-3 py-1.5 text-xs">
                {d.host}:{d.port}
                {d.database && d.database !== d.name && (
                  <span className="ml-2 text-text-dim">→ {d.database}</span>
                )}
              </td>
              <td className="px-3 py-1.5 text-xs text-text-muted">{d.force_user || "—"}</td>
              <td className="px-3 py-1.5 text-xs">{d.pool_mode}</td>
              <td className="px-3 py-1.5 text-right">{d.pool_size}</td>
              <td className="px-3 py-1.5 text-right text-text-dim">{d.min_pool_size}</td>
              <td className="px-3 py-1.5 text-right">
                {d.current_connections} / {d.max_connections}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </PanelShell>
  );
}

function UsersPanel() {
  const poll = useEndpoint<UsersDto>("/api/users", SLOW_MS);
  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-6 py-2 text-left">User</th>
            <th className="px-3 py-2 text-left">Pool mode</th>
          </tr>
        </thead>
        <tbody>
          {poll.data?.users.map((u, i) => (
            <tr key={`${u.name}-${i}`} className="border-b border-border/40 hover:bg-surface-2">
              <td className="px-6 py-1.5 font-mono text-xs">{u.name}</td>
              <td className="px-3 py-1.5 text-xs">{u.pool_mode}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </PanelShell>
  );
}

function SocketsPanel() {
  const poll = useEndpoint<SocketsDto>("/api/sockets", FAST_MS);
  if (!poll.data && !poll.error) {
    return <p className="px-6 py-4 text-sm text-text-dim">loading…</p>;
  }
  if (poll.error) {
    return (
      <p className="px-6 py-4 text-sm text-text-dim">
        Socket counts are Linux-only. On macOS/Windows builds this card stays empty; if you are on Linux and seeing this, the request failed: {poll.error.message}
      </p>
    );
  }
  const s = poll.data!;
  return (
    <div className="grid grid-cols-1 gap-6 px-6 py-4 md:grid-cols-3">
      <SocketGroup title="TCP">
        <KV label="established" value={s.tcp.established} highlight={s.tcp.established === 0 ? "warn" : null} />
        <KV label="time-wait" value={s.tcp.time_wait} />
        <KV label="close-wait" value={s.tcp.close_wait} highlight={s.tcp.close_wait > 0 ? "warn" : null} />
        <KV label="listen" value={s.tcp.listen} />
      </SocketGroup>
      <SocketGroup title="TCP6">
        <KV label="established" value={s.tcp6.established} />
        <KV label="time-wait" value={s.tcp6.time_wait} />
        <KV label="close-wait" value={s.tcp6.close_wait} />
      </SocketGroup>
      <SocketGroup title="Unix stream">
        <KV label="connected" value={s.unix_stream.connected} />
        <KV label="unconnected" value={s.unix_stream.unconnected} />
        <KV label="connecting" value={s.unix_stream.connecting} />
        <KV label="disconnecting" value={s.unix_stream.disconnecting} />
      </SocketGroup>
    </div>
  );
}

function SocketGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div>
      <div className="mb-2 text-[10px] font-semibold uppercase tracking-wide text-text-dim">
        {title}
      </div>
      <dl className="space-y-1 text-sm tabular">{children}</dl>
    </div>
  );
}

function KV({
  label,
  value,
  highlight,
}: {
  label: string;
  value: number;
  highlight?: "warn" | "crit" | null;
}) {
  return (
    <div className="flex items-baseline justify-between">
      <dt className="text-text-muted">{label}</dt>
      <dd
        className={
          highlight === "crit" ? "text-danger" : highlight === "warn" ? "text-warning" : "text-text"
        }
      >
        {value}
      </dd>
    </div>
  );
}

function StartupParametersPanel() {
  // Reuses /api/pools — each pool already carries its resolved startup
  // parameters cascade in the same payload that drives the Pools list,
  // so no extra endpoint is needed for the overview. Slow poll: declared
  // values change only on SIGHUP and the operator does not need 1.5 s
  // freshness here.
  const poll = useEndpoint<PoolsDto>("/api/pools", SLOW_MS);
  const rows = useMemo(() => {
    const out: {
      poolId: string;
      parameter: string;
      value: string;
      source: string;
      state: string;
    }[] = [];
    for (const pool of poll.data?.pools ?? []) {
      for (const p of pool.startup_parameters ?? []) {
        out.push({
          poolId: pool.id,
          parameter: p.parameter,
          value: p.value ?? "***",
          source: p.source,
          state: p.state ?? "applied",
        });
      }
    }
    return out;
  }, [poll.data]);
  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      {rows.length === 0 ? (
        <p className="px-6 py-4 text-sm text-text-dim">
          No pool has operator-supplied startup parameters. Configure them under
          <code className="mx-1 font-mono">[pools.&lt;db&gt;.&lt;user&gt;] startup_parameters</code>
          or
          <code className="mx-1 font-mono">[databases.&lt;db&gt;] startup_parameters</code>
          to override the libpq defaults for new backend connections.
        </p>
      ) : (
        <table className="w-full text-sm tabular">
          <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
            <tr>
              <th className="px-6 py-2 text-left">Pool</th>
              <th className="px-3 py-2 text-left">Parameter</th>
              <th className="px-3 py-2 text-left">Value</th>
              <th className="px-3 py-2 text-left">Source</th>
              <th className="px-3 py-2 text-left">State</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => {
              const stateTone =
                r.state === "applied"
                  ? "text-text-dim"
                  : r.state === "dropped_due_to_budget"
                    ? "text-danger"
                    : "text-warning";
              return (
                <tr
                  key={`${r.poolId}-${r.parameter}`}
                  className="border-b border-border/40 hover:bg-surface-2"
                >
                  <td className="px-6 py-1.5 font-mono text-xs">{r.poolId}</td>
                  <td className="px-3 py-1.5 font-mono text-xs">{r.parameter}</td>
                  <td className="px-3 py-1.5 font-mono text-xs">{r.value}</td>
                  <td className="px-3 py-1.5 text-xs text-text-muted">{r.source}</td>
                  <td className={`px-3 py-1.5 text-xs ${stateTone}`}>{r.state}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </PanelShell>
  );
}

function PoolScalingPanel() {
  const poll = useEndpoint<PoolScalingDto>("/api/pool_scaling", FAST_MS);
  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-6 py-2 text-left">Pool</th>
            <th className="px-3 py-2 text-right">In-flight</th>
            <th className="px-3 py-2 text-right">Creates</th>
            <th className="px-3 py-2 text-right">Gate waits</th>
            <th className="px-3 py-2 text-right">Budget ex</th>
            <th className="px-3 py-2 text-right">Antic notify</th>
            <th className="px-3 py-2 text-right">Antic timeout</th>
            <th className="px-3 py-2 text-right">Fallback</th>
          </tr>
        </thead>
        <tbody>
          {poll.data?.pools.map((row) => (
            <tr
              key={`${row.user}@${row.database}`}
              className="border-b border-border/40 hover:bg-surface-2"
            >
              <td className="px-6 py-1.5 font-mono text-xs">
                {row.user}@{row.database}
              </td>
              <td className="px-3 py-1.5 text-right">{row.inflight}</td>
              <td className="px-3 py-1.5 text-right">{row.creates}</td>
              <td className="px-3 py-1.5 text-right">{row.gate_waits}</td>
              <td
                className={`px-3 py-1.5 text-right ${row.gate_budget_ex > 0 ? "text-warning" : ""}`}
              >
                {row.gate_budget_ex}
              </td>
              <td className="px-3 py-1.5 text-right">{row.antic_notify}</td>
              <td
                className={`px-3 py-1.5 text-right ${row.antic_timeout > 0 ? "text-warning" : ""}`}
              >
                {row.antic_timeout}
              </td>
              <td
                className={`px-3 py-1.5 text-right ${row.create_fallback > 0 ? "text-warning" : ""}`}
              >
                {row.create_fallback}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </PanelShell>
  );
}

function PoolCoordinatorPanel() {
  const poll = useEndpoint<PoolCoordinatorDto>("/api/pool_coordinator", FAST_MS);
  return (
    <PanelShell loading={!poll.data} error={poll.error}>
      <table className="w-full text-sm tabular">
        <thead className="bg-surface text-text-muted text-xs uppercase tracking-wide">
          <tr>
            <th className="px-6 py-2 text-left">Database</th>
            <th className="px-3 py-2 text-right">Connections</th>
            <th className="px-3 py-2 text-right">Reserve used / size</th>
            <th className="px-3 py-2 text-right">Reserve acq</th>
            <th className="px-3 py-2 text-right">Evictions</th>
            <th className="px-3 py-2 text-right">Exhaustions</th>
          </tr>
        </thead>
        <tbody>
          {poll.data?.databases.map((row) => {
            const sat = row.max_db_conn > 0 ? row.current / row.max_db_conn : 0;
            return (
              <tr key={row.database} className="border-b border-border/40 hover:bg-surface-2">
                <td className="px-6 py-1.5 font-mono text-xs">{row.database}</td>
                <td
                  className={`px-3 py-1.5 text-right ${
                    sat >= 0.9 ? "text-danger" : sat >= 0.7 ? "text-warning" : ""
                  }`}
                >
                  {row.current} / {row.max_db_conn}{" "}
                  <span className="text-text-dim">({(sat * 100).toFixed(0)}%)</span>
                </td>
                <td className="px-3 py-1.5 text-right">
                  {row.reserve_used} / {row.reserve_size}
                </td>
                <td className="px-3 py-1.5 text-right">{row.reserve_acq}</td>
                <td className="px-3 py-1.5 text-right">{row.evictions}</td>
                <td
                  className={`px-3 py-1.5 text-right ${row.exhaustions > 0 ? "text-danger" : ""}`}
                >
                  {row.exhaustions}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </PanelShell>
  );
}
