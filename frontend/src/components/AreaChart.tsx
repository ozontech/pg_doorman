import { useMemo } from "react";
import type { Options } from "uplot";
import { Chart } from "./Chart";

interface AreaChartProps {
  /** First entry is the time axis; remaining are stacked numeric series. */
  data: [number[], ...number[][]];
  /** Per-series labels in the same order as data[1..]. */
  labels: string[];
  /** Per-series fill colors. */
  fills: string[];
  height?: number;
  syncKey?: string;
}

/**
 * Stacked area chart. uPlot does not provide native stacking — we precompute
 * cumulative series before passing them in, so each band paints on top of
 * the previous one.
 */
export function AreaChart({ data, labels, fills, height = 200, syncKey }: AreaChartProps) {
  const stacked = useMemo<[number[], ...number[][]]>(() => {
    const xs = data[0];
    const cumulative: number[][] = [];
    for (let i = 1; i < data.length; i++) {
      const prev = cumulative[cumulative.length - 1] ?? new Array(xs.length).fill(0);
      const next = data[i].map((v, idx) => (prev[idx] ?? 0) + v);
      cumulative.push(next);
    }
    return [xs, ...cumulative] as [number[], ...number[][]];
  }, [data]);

  const options: Options = useMemo(
    () => ({
      width: 1024,
      height,
      cursor: syncKey ? { sync: { key: syncKey } } : undefined,
      legend: { show: false },
      scales: { y: { range: (_u, _min, max) => [0, max] } },
      axes: [
        { stroke: "rgb(138 147 164)", grid: { stroke: "rgb(35 42 54 / 0.4)" } },
        { stroke: "rgb(138 147 164)", grid: { stroke: "rgb(35 42 54 / 0.4)" } },
      ],
      series: [
        {},
        ...labels.map((label, i) => ({
          label,
          stroke: fills[i],
          fill: fills[i],
          width: 1,
          // Paint on top — uPlot draws series in order; we already stacked.
        })),
      ],
    }),
    [labels, fills, height, syncKey],
  );

  // Static legend above the canvas. uPlot's built-in legend renders inline
  // values which we don't need; a fixed colour-swatch row tells operators
  // which band is which without forcing them to remember the stack order.
  const totals = useMemo(() => data.slice(1).map((s) => s[s.length - 1] ?? 0), [data]);

  return (
    <div className="w-full overflow-hidden">
      <div className="mb-2 flex flex-wrap gap-x-4 gap-y-1 px-2 text-xs text-text-muted">
        {labels.map((l, i) => (
          <span key={l} className="inline-flex items-center gap-1.5">
            <span
              aria-hidden
              className="inline-block h-2 w-2 rounded-sm"
              style={{ background: fills[i] }}
            />
            <span>{l}</span>
            <span className="tabular text-text-dim">{Math.round(totals[i] ?? 0)}</span>
          </span>
        ))}
      </div>
      <Chart data={stacked} options={options} />
    </div>
  );
}
