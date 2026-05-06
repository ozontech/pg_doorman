import type { HealthState } from "../lib/thresholds";
import type { OverviewDto, PoolsDto } from "../types";

const PILL_STYLES: Record<HealthState["state"], string> = {
  ok: "bg-success/20 text-success",
  degraded: "bg-warning/20 text-warning",
  critical: "bg-danger/20 text-danger",
};

const PILL_LABELS: Record<HealthState["state"], string> = {
  ok: "OK",
  degraded: "DEGRADED",
  critical: "CRITICAL",
};

interface HealthPillProps {
  health: HealthState;
  lastUpdated: number | null;
  overview: OverviewDto | null;
  pools: PoolsDto | null;
  /** errors/s derived on the page side. */
  errorsPerSecond: number | null;
}

/**
 * Health bar: status pill + a compact strip of operator-relevant chips
 * (pools, paused, errors/s, prepared hit rate, active clients,
 * busy/total servers, waiting). Mirrors spec §15.1. Renders inline within
 * a max-width container — does not stretch across the viewport.
 */
export function HealthPill({
  health,
  lastUpdated,
  overview,
  pools,
  errorsPerSecond,
}: HealthPillProps) {
  const ageSeconds =
    lastUpdated === null ? null : Math.max(0, Math.round((Date.now() - lastUpdated) / 1000));
  const hitRate =
    overview && overview.prepared_hits_total + overview.prepared_misses_total > 0
      ? overview.prepared_hits_total /
        (overview.prepared_hits_total + overview.prepared_misses_total)
      : null;
  const totalServers = overview ? overview.active_servers + overview.idle_servers : 0;
  return (
    <div className="rounded-md border border-border bg-surface px-4 py-3">
      <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
        <span
          className={`px-2.5 py-0.5 rounded text-xs font-semibold ${PILL_STYLES[health.state]}`}
        >
          ● {PILL_LABELS[health.state]}
        </span>
        {pools && <Chip label="pools" value={pools.pools.length.toString()} />}
        {overview && overview.pools_paused > 0 && (
          <Chip label="paused" value={overview.pools_paused.toString()} tone="warn" />
        )}
        {errorsPerSecond !== null && (
          <Chip
            label="errors/s"
            value={errorsPerSecond.toFixed(2)}
            tone={
              errorsPerSecond > 10 ? "crit" : errorsPerSecond > 1 ? "warn" : undefined
            }
          />
        )}
        {hitRate !== null && (
          <Chip
            label="prepared hit"
            value={`${(hitRate * 100).toFixed(1)}%`}
            tone={hitRate < 0.8 ? "crit" : hitRate < 0.95 ? "warn" : undefined}
          />
        )}
        {overview && (
          <Chip label="active" value={overview.active_clients.toString()} />
        )}
        {overview && (
          <Chip
            label="servers"
            value={`${overview.active_servers}/${totalServers}`}
          />
        )}
        {overview && (
          <Chip
            label="waiting"
            value={overview.waiting_clients.toString()}
            tone={overview.waiting_clients > 0 ? "warn" : undefined}
          />
        )}
        {health.reason && (
          <span className="text-text-muted text-xs italic">— {health.reason}</span>
        )}
        <span className="ml-auto text-xs text-text-dim tabular">
          {ageSeconds === null ? "no data" : `updated ${ageSeconds}s ago`}
        </span>
      </div>
    </div>
  );
}

function Chip({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: "warn" | "crit";
}) {
  const valueColor = tone === "crit" ? "text-danger" : tone === "warn" ? "text-warning" : "text-text";
  return (
    <span className="flex items-baseline gap-1.5 text-xs">
      <span className="text-text-dim uppercase tracking-wide">{label}</span>
      <span className={`font-mono font-semibold ${valueColor}`}>{value}</span>
    </span>
  );
}
