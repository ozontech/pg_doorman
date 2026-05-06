import { useEffect, useMemo, useRef, useState } from "react";
import uPlot, { type Options } from "uplot";
import "uplot/dist/uPlot.min.css";

export interface ChartEvent {
  /// Unix timestamp in seconds (uPlot convention). Frontend converts ms → s.
  ts: number;
  /// Short label drawn near the line (e.g. "RELOAD"). Optional.
  label?: string;
}

interface SparklineProps {
  label: string;
  valueText: string;
  series: [number[], number[]];
  warn?: number;
  crit?: number;
  logY?: boolean;
  syncKey?: string;
  events?: ChartEvent[];
}

const HEIGHT_PX = 64;
const STROKE = "rgb(34 184 207)";
const WARN_STROKE = "rgb(245 165 36 / 0.55)";
const CRIT_STROKE = "rgb(229 72 77 / 0.55)";

/**
 * uPlot-backed sparkline that fills its container width via ResizeObserver.
 * The label and value sit above the chart; threshold lines are painted in
 * the draw hook so they survive setData without a chart rebuild.
 */
export function Sparkline({
  label,
  valueText,
  series,
  warn,
  crit,
  logY,
  syncKey,
  events,
}: SparklineProps) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const plotRef = useRef<uPlot | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    if (!wrapRef.current) return;
    const ro = new ResizeObserver((entries) => {
      const w = Math.max(40, Math.floor(entries[0].contentRect.width));
      setWidth(w);
    });
    ro.observe(wrapRef.current);
    return () => ro.disconnect();
  }, []);

  const options: Options = useMemo(
    () => ({
      width: width || 200,
      height: HEIGHT_PX,
      cursor: syncKey ? { sync: { key: syncKey } } : undefined,
      legend: { show: false },
      scales: { y: logY ? { distr: 3 } : { auto: true } },
      axes: [{ show: false }, { show: false }],
      series: [{}, { stroke: STROKE, width: 1.5 }],
      hooks: {
        draw: [
          (u: uPlot) => {
            const ctx = u.ctx;
            const drawLine = (yVal: number, color: string) => {
              const yPx = u.valToPos(yVal, "y", true);
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
            if (warn !== undefined) drawLine(warn, WARN_STROKE);
            if (crit !== undefined) drawLine(crit, CRIT_STROKE);
            // Event annotations: thin amber vertical line per /api/events
            // entry inside the visible window.
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
    [width, warn, crit, logY, syncKey, events],
  );

  // Create plot only after width is known.
  useEffect(() => {
    if (!containerRef.current) return;
    if (width === 0) return;
    plotRef.current = new uPlot(options, series, containerRef.current);
    return () => {
      plotRef.current?.destroy();
      plotRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [options]);

  useEffect(() => {
    plotRef.current?.setData(series);
  }, [series]);

  return (
    <div
      ref={wrapRef}
      className="flex flex-col gap-1 rounded-md border border-border bg-surface p-3"
    >
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-[10px] uppercase tracking-[0.18em] text-text-dim">{label}</span>
        <span
          className="whitespace-nowrap truncate font-mono text-sm font-semibold text-text tabular"
          title={valueText}
        >
          {valueText}
        </span>
      </div>
      <div ref={containerRef} className="w-full" />
    </div>
  );
}
