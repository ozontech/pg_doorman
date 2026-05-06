interface HeatmapRowProps {
  label: string;
  /** Latest 60 saturation values, oldest-first. Missing tail is rendered empty. */
  cells: (number | null)[];
  capacity: number;
}

const CELL_WIDTH = 12;
const CELL_HEIGHT = 14;
const CELL_GAP = 1;

function colorFor(saturation: number): string {
  if (saturation >= 0.9) return "rgb(229 72 77)"; // danger
  if (saturation >= 0.7) return "rgb(245 165 36)"; // warning
  return "rgb(45 194 107)"; // success
}

function HeatmapRow({ label, cells, capacity }: HeatmapRowProps) {
  return (
    <div className="flex items-center gap-3 px-4 py-1.5 hover:bg-surface-2">
      <span className="w-48 truncate text-sm text-text">{label}</span>
      <span className="w-16 text-right text-xs text-text-dim tabular">{capacity} max</span>
      <div className="flex gap-px">
        {cells.map((cell, i) => (
          <div
            key={i}
            className="rounded-sm"
            style={{
              width: CELL_WIDTH,
              height: CELL_HEIGHT,
              marginRight: CELL_GAP,
              background: cell === null ? "rgb(35 42 54)" : colorFor(cell),
              opacity: cell === null ? 0.3 : 1,
            }}
            title={cell === null ? "no sample" : `${(cell * 100).toFixed(0)}%`}
          />
        ))}
      </div>
    </div>
  );
}

interface HeatmapProps {
  /** Each row is one pool with its rolling cells. */
  rows: { label: string; cells: (number | null)[]; capacity: number }[];
  /** Truncate to the first N rows; render a footer when there are more. */
  maxRows?: number;
}

export function Heatmap({ rows, maxRows = 30 }: HeatmapProps) {
  const visible = rows.slice(0, maxRows);
  const truncated = rows.length - visible.length;
  return (
    <div className="border-b border-border py-2">
      <div className="flex items-center px-4 py-1 text-xs text-text-muted uppercase tracking-wide">
        <span className="w-48">Pool</span>
        <span className="w-16 text-right">Capacity</span>
        <span className="ml-3">Last 60 samples · saturation</span>
      </div>
      {visible.map((r) => (
        <HeatmapRow key={r.label} {...r} />
      ))}
      {truncated > 0 && (
        <div className="px-4 py-1 text-xs text-text-dim">+{truncated} more pools (truncated)</div>
      )}
    </div>
  );
}
