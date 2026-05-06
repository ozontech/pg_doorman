// Pool saturation heatmap — one row per pool, 60 cells × 1.5 s window. The
// previous version relied on the browser's native title="" tooltip; that
// has a ~1 s delay and cannot show the cell timestamp. The custom overlay
// renders instantly, says when the sample was taken, and turns the row
// label into a link to the pool drilldown.

import { useState, type CSSProperties, type MouseEvent } from "react";
import { useNavigate } from "react-router-dom";

interface HeatmapRowProps {
  label: string;
  /** Latest 60 saturation values, oldest-first. Missing tail is rendered empty. */
  cells: (number | null)[];
  capacity: number;
  pollIntervalMs: number;
}

const CELL_WIDTH = 12;
const CELL_HEIGHT = 14;
const CELL_GAP = 1;

function colorFor(saturation: number): string {
  if (saturation >= 0.9) return "rgb(229 72 77)"; // danger
  if (saturation >= 0.7) return "rgb(245 165 36)"; // warning
  return "rgb(45 194 107)"; // success
}

function HeatmapRow({ label, cells, capacity, pollIntervalMs }: HeatmapRowProps) {
  const navigate = useNavigate();
  const [hover, setHover] = useState<{ idx: number; x: number; y: number } | null>(null);

  const onCellEnter = (i: number, e: MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    setHover({ idx: i, x: rect.left + rect.width / 2, y: rect.top });
  };

  return (
    <div className="flex items-center gap-3 px-4 py-1.5 hover:bg-surface-2">
      <button
        type="button"
        onClick={() => navigate(`/pools/${encodeURIComponent(label)}`)}
        className="w-48 truncate text-left text-sm text-text hover:text-accent"
        title="Open pool detail"
      >
        {label}
      </button>
      <span className="w-16 text-right text-xs text-text-dim tabular">{capacity} max</span>
      <div className="relative flex gap-px">
        {cells.map((cell, i) => (
          <div
            key={i}
            className="cursor-pointer rounded-sm"
            style={{
              width: CELL_WIDTH,
              height: CELL_HEIGHT,
              marginRight: CELL_GAP,
              background: cell === null ? "rgb(35 42 54)" : colorFor(cell),
              opacity: cell === null ? 0.3 : 1,
            }}
            onMouseEnter={(e) => onCellEnter(i, e)}
            onMouseLeave={() => setHover(null)}
          />
        ))}
        {hover && (
          <div
            className="pointer-events-none fixed z-50 -translate-x-1/2 -translate-y-full rounded border border-border-strong bg-surface px-2 py-1 text-xs text-text shadow-lg"
            style={{ left: hover.x, top: hover.y - 6 } as CSSProperties}
          >
            {(() => {
              const cell = cells[hover.idx];
              const ageSec = ((cells.length - 1 - hover.idx) * pollIntervalMs) / 1000;
              const ageLabel = ageSec === 0 ? "now" : `${Math.round(ageSec)} s ago`;
              if (cell === null) {
                return <span className="text-text-dim">no sample · {ageLabel}</span>;
              }
              return (
                <span>
                  <span className="font-mono">{(cell * 100).toFixed(0)}%</span>{" "}
                  <span className="text-text-dim">· {ageLabel}</span>
                </span>
              );
            })()}
          </div>
        )}
      </div>
    </div>
  );
}

interface HeatmapProps {
  /** Each row is one pool with its rolling cells. */
  rows: { label: string; cells: (number | null)[]; capacity: number }[];
  /** Truncate to the first N rows; render a footer when there are more. */
  maxRows?: number;
  /** Poll interval used to label "Ns ago" on the hover overlay. */
  pollIntervalMs?: number;
}

export function Heatmap({ rows, maxRows = 30, pollIntervalMs = 1500 }: HeatmapProps) {
  const visible = rows.slice(0, maxRows);
  const truncated = rows.length - visible.length;
  return (
    <div className="border-b border-border py-2">
      <div className="flex items-center px-4 py-1 text-xs text-text-muted uppercase tracking-wide">
        <span className="w-48">Pool</span>
        <span className="w-16 text-right">Capacity</span>
        <span className="ml-3 flex items-center gap-3">
          <span>Last 60 samples · saturation</span>
          <span className="inline-flex items-center gap-2 normal-case tracking-normal text-[10px]">
            <span className="inline-block h-2 w-2" style={{ background: "rgb(45 194 107)" }} />{" "}
            &lt; 70%
            <span className="inline-block h-2 w-2" style={{ background: "rgb(245 165 36)" }} />{" "}
            70–89%
            <span className="inline-block h-2 w-2" style={{ background: "rgb(229 72 77)" }} />{" "}
            ≥ 90%
          </span>
        </span>
      </div>
      {visible.map((r) => (
        <HeatmapRow key={r.label} {...r} pollIntervalMs={pollIntervalMs} />
      ))}
      {truncated > 0 && (
        <div className="px-4 py-1 text-xs text-text-dim">+{truncated} more pools (truncated)</div>
      )}
    </div>
  );
}
