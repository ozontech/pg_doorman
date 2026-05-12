import { useMemo, useRef, useState } from "react";
import type { Options } from "uplot";
import type uPlot from "uplot";
import { Chart } from "./Chart";
import type { ChartEvent } from "./Sparkline";

interface AreaChartProps {
  /** First entry is the time axis; remaining are stacked numeric series. */
  data: [number[], ...number[][]];
  /** Per-series labels in the same order as data[1..]. */
  labels: string[];
  /** Per-series fill colors. */
  fills: string[];
  height?: number;
  syncKey?: string;
  events?: ChartEvent[];
}

/**
 * Stacked area chart. uPlot does not provide native stacking — we precompute
 * cumulative series before passing them in, so each band paints on top of
 * the previous one.
 */
export function AreaChart({
  data,
  labels,
  fills,
  height = 200,
  syncKey,
  events,
}: AreaChartProps) {
  const [hover, setHover] = useState<{ ts: number; values: number[] } | null>(null);
  // Live ref for the admin-events annotations so the draw hook reads the
  // latest entries without forcing options to re-create on every event
  // poll — that re-creation tore down the plot every 3 s.
  const eventsRef = useRef(events);
  eventsRef.current = events;
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
      cursor: {
        sync: syncKey ? { key: syncKey } : undefined,
        points: { size: 4 },
      },
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
          // Only the bottom series fills from the X-axis baseline.
          // Higher series get coloured via `bands` (between series i
          // and i-1) so a top layer whose values are zero everywhere
          // does not repaint the layers below it with its own colour.
          fill: i === 0 ? fills[i] : undefined,
          width: 1,
        })),
      ],
      bands: labels.slice(1).map((_, idx) => ({
        // [top, bottom]: paint the area between series idx+2 and idx+1
        // with the top series' colour.
        series: [idx + 2, idx + 1] as [number, number],
        fill: fills[idx + 1],
      })),
      hooks: {
        setCursor: [
          (u: uPlot) => {
            const idx = u.cursor.idx;
            if (idx == null || idx < 0) {
              setHover(null);
              return;
            }
            const xs = u.data[0] as number[];
            const ts = xs[idx];
            if (ts == null) {
              setHover(null);
              return;
            }
            // Reverse the cumulative stacking so each label sees its own
            // value rather than the running total.
            const values: number[] = [];
            let prev = 0;
            for (let i = 1; i < u.data.length; i++) {
              const v = (u.data[i] as number[])[idx];
              values.push(v - prev);
              prev = v;
            }
            setHover({ ts, values });
          },
        ],
        draw: [
          (u: uPlot) => {
            const live = eventsRef.current;
            if (!live || live.length === 0) return;
            const ctx = u.ctx;
            ctx.save();
            ctx.strokeStyle = "rgb(255 176 0 / 0.55)";
            ctx.setLineDash([]);
            ctx.lineWidth = 1;
            for (const ev of live) {
              const xPx = u.valToPos(ev.ts, "x", true);
              if (!Number.isFinite(xPx)) continue;
              if (xPx < u.bbox.left || xPx > u.bbox.left + u.bbox.width) continue;
              ctx.beginPath();
              ctx.moveTo(xPx, u.bbox.top);
              ctx.lineTo(xPx, u.bbox.top + u.bbox.height);
              ctx.stroke();
            }
            ctx.restore();
          },
        ],
      },
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
      {data[0].length >= 2 ? (
        <Chart data={stacked} options={options} />
      ) : (
        <div
          style={{ height }}
          className="flex items-center justify-center text-xs text-text-dim"
        >
          collecting samples · {data[0].length}/120
        </div>
      )}
      <div className="mt-1 px-2 text-xs text-text-dim tabular">
        {hover ? (
          <span className="flex flex-wrap gap-x-4">
            <span>{new Date(hover.ts * 1000).toLocaleTimeString()}</span>
            {labels.map((l, i) => (
              <span key={l}>
                <span style={{ color: fills[i] }}>{l}</span>{" "}
                <span className="text-text">{Math.round(hover.values[i] ?? 0)}</span>
              </span>
            ))}
          </span>
        ) : (
          <span>hover for per-bucket values</span>
        )}
      </div>
    </div>
  );
}
