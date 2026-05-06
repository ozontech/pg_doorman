import type { HealthState } from "../lib/thresholds";

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

export function HealthPill({
  health,
  lastUpdated,
}: {
  health: HealthState;
  lastUpdated: number | null;
}) {
  const ageSeconds =
    lastUpdated === null ? null : Math.max(0, Math.round((Date.now() - lastUpdated) / 1000));
  return (
    <div className="flex items-center gap-3 px-4 py-3 border-b border-border bg-surface">
      <span
        className={`px-2 py-0.5 rounded text-xs font-semibold ${PILL_STYLES[health.state]}`}
      >
        ● {PILL_LABELS[health.state]}
      </span>
      {health.reason && (
        <span className="text-text-muted text-sm italic">{health.reason}</span>
      )}
      <span className="ml-auto text-xs text-text-dim tabular">
        {ageSeconds === null ? "no data" : `Updated ${ageSeconds}s ago`}
      </span>
    </div>
  );
}
