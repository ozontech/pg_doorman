import { useMemo, useState } from "react";
import type { Options } from "uplot";
import type uPlot from "uplot";
import { Chart } from "./Chart";
import type { ChartEvent } from "./Sparkline";

interface DualAxisChartProps {
  /** [time, leftSeries, rightSeries] */
  data: [number[], number[], number[]];
  leftLabel: string;
  rightLabel: string;
  leftStroke: string;
  rightStroke: string;
  rightLogScale?: boolean;
  rightWarn?: number;
  rightCrit?: number;
  height?: number;
  syncKey?: string;
  events?: ChartEvent[];
}

export function DualAxisChart({
  data,
  leftLabel,
  rightLabel,
  leftStroke,
  rightStroke,
  rightLogScale,
  rightWarn,
  rightCrit,
  height = 200,
  syncKey,
  events,
}: DualAxisChartProps) {
  const [hover, setHover] = useState<{ ts: number; left: number; right: number } | null>(null);
  const options: Options = useMemo(
    () => ({
      width: 1024,
      height,
      cursor: {
        sync: syncKey ? { key: syncKey } : undefined,
        points: { size: 4 },
      },
      legend: { show: false },
      scales: {
        y: { auto: true },
        y2: rightLogScale ? { distr: 3 } : { auto: true },
      },
      axes: [
        { stroke: "rgb(138 147 164)", grid: { stroke: "rgb(35 42 54 / 0.4)" } },
        {
          stroke: leftStroke,
          grid: { stroke: "rgb(35 42 54 / 0.4)" },
          scale: "y",
          label: leftLabel,
        },
        { stroke: rightStroke, scale: "y2", side: 1, grid: { show: false }, label: rightLabel },
      ],
      series: [
        {},
        { label: leftLabel, stroke: leftStroke, width: 1.5, scale: "y" },
        { label: rightLabel, stroke: rightStroke, width: 1.5, scale: "y2" },
      ],
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
            const left = (u.data[1] as number[])[idx];
            const right = (u.data[2] as number[])[idx];
            if (ts == null) {
              setHover(null);
              return;
            }
            setHover({ ts, left, right });
          },
        ],
        draw: [
          (u: uPlot) => {
            const ctx = u.ctx;
            const drawLine = (yVal: number, color: string) => {
              const yPx = u.valToPos(yVal, "y2", true);
              if (!Number.isFinite(yPx)) return;
              ctx.save();
              ctx.strokeStyle = color;
              ctx.setLineDash([3, 3]);
              ctx.lineWidth = 1;
              ctx.beginPath();
              ctx.moveTo(u.bbox.left, yPx);
              ctx.lineTo(u.bbox.left + u.bbox.width, yPx);
              ctx.stroke();
              ctx.restore();
            };
            if (rightWarn !== undefined) drawLine(rightWarn, "rgb(245 165 36 / 0.6)");
            if (rightCrit !== undefined) drawLine(rightCrit, "rgb(229 72 77 / 0.6)");
            if (events && events.length > 0) {
              ctx.save();
              ctx.strokeStyle = "rgb(255 176 0 / 0.55)";
              ctx.setLineDash([]);
              ctx.lineWidth = 1;
              for (const ev of events) {
                const xPx = u.valToPos(ev.ts, "x", true);
                if (!Number.isFinite(xPx)) continue;
                if (xPx < u.bbox.left || xPx > u.bbox.left + u.bbox.width) continue;
                ctx.beginPath();
                ctx.moveTo(xPx, u.bbox.top);
                ctx.lineTo(xPx, u.bbox.top + u.bbox.height);
                ctx.stroke();
              }
              ctx.restore();
            }
          },
        ],
      },
    }),
    [
      leftLabel,
      rightLabel,
      leftStroke,
      rightStroke,
      rightLogScale,
      rightWarn,
      rightCrit,
      height,
      syncKey,
      events,
    ],
  );

  const leftLatest = data[1][data[1].length - 1] ?? 0;
  const rightLatest = data[2][data[2].length - 1] ?? 0;

  return (
    <div className="w-full overflow-hidden">
      <div className="mb-2 flex flex-wrap gap-x-4 gap-y-1 px-2 text-xs text-text-muted">
        <span className="inline-flex items-center gap-1.5">
          <span
            aria-hidden
            className="inline-block h-2 w-2 rounded-sm"
            style={{ background: leftStroke }}
          />
          <span>{leftLabel} (left)</span>
          <span className="tabular text-text-dim">{Math.round(leftLatest)}</span>
        </span>
        <span className="inline-flex items-center gap-1.5">
          <span
            aria-hidden
            className="inline-block h-2 w-2 rounded-sm"
            style={{ background: rightStroke }}
          />
          <span>{rightLabel} (right)</span>
          <span className="tabular text-text-dim">{Math.round(rightLatest)}</span>
        </span>
        {rightWarn !== undefined && (
          <span className="text-warning">warn ≥ {rightWarn}</span>
        )}
        {rightCrit !== undefined && (
          <span className="text-danger">crit ≥ {rightCrit}</span>
        )}
      </div>
      <Chart data={data} options={options} />
      <div className="mt-1 px-2 text-xs text-text-dim tabular">
        {hover ? (
          <span className="flex flex-wrap gap-x-4">
            <span>{new Date(hover.ts * 1000).toLocaleTimeString()}</span>
            <span style={{ color: leftStroke }}>
              {leftLabel} <span className="text-text">{Math.round(hover.left)}</span>
            </span>
            <span style={{ color: rightStroke }}>
              {rightLabel} <span className="text-text">{Math.round(hover.right)}</span>
            </span>
          </span>
        ) : (
          <span>hover for value at cursor</span>
        )}
      </div>
    </div>
  );
}
