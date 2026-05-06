import { useEffect, useMemo, useRef, useState } from "react";
import uPlot, { type Options } from "uplot";
import "uplot/dist/uPlot.min.css";

interface SparklineProps {
  label: string;
  valueText: string;
  series: [number[], number[]];
  warn?: number;
  crit?: number;
  logY?: boolean;
  syncKey?: string;
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
          },
        ],
      },
    }),
    [width, warn, crit, logY, syncKey],
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
        <span className="font-mono text-base font-semibold text-text tabular">{valueText}</span>
      </div>
      <div ref={containerRef} className="w-full" />
    </div>
  );
}
