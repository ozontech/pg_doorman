import { useEffect, useMemo, useRef, useState } from "react";
import uPlot, { type Options } from "uplot";
import "uplot/dist/uPlot.min.css";
import { InfoLabel } from "./InfoLabel";

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
  /// Optional one-sentence explanation, rendered as a hover tooltip on the
  /// title via InfoLabel. Operators new to pg_doorman want to know what
  /// the sparkline measures and what healthy looks like without leaving
  /// for the docs.
  tip?: string;
}

const HEIGHT_PX = 64;
const STROKE = "rgb(34 184 207)";
const WARN_STROKE = "rgb(245 165 36 / 0.55)";
const CRIT_STROKE = "rgb(229 72 77 / 0.55)";

function formatHoverValue(v: number): string {
  if (!Number.isFinite(v)) return "—";
  if (Math.abs(v) >= 1000) return v.toFixed(0);
  if (Math.abs(v) >= 10) return v.toFixed(1);
  return v.toFixed(2);
}

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
  tip,
}: SparklineProps) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const plotRef = useRef<uPlot | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);
  // Cursor readout — populated by uPlot's setCursor hook on every mouse
  // move. Null means the cursor left the canvas.
  const [hover, setHover] = useState<{ ts: number; value: number } | null>(null);

  // Mirror `events` in a ref so the draw hook reads the latest array
  // without forcing the options useMemo to re-create on every event poll.
  // Earlier the options dep array included `events` directly; the
  // /api/events poll handed us a new array reference every 3 s, which
  // destroyed and recreated the plot — visually the chart blanked for a
  // frame and the operator saw the dashboard "flicker on its own".
  const eventsRef = useRef(events);
  eventsRef.current = events;

  // Width is also reffed: ResizeObserver updates a state but only to
  // gate the first plot creation, never to recreate. Live size changes
  // route through plot.setSize() in a separate effect.
  const widthRef = useRef(width);
  widthRef.current = width;

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
      // First creation uses whatever width is current; later resize calls
      // setSize() against the live plot instead of recreating it.
      width: widthRef.current || 200,
      height: HEIGHT_PX,
      cursor: {
        sync: syncKey ? { key: syncKey } : undefined,
        points: { size: 5 },
      },
      legend: { show: false },
      scales: { y: logY ? { distr: 3 } : { auto: true } },
      axes: [{ show: false }, { show: false }],
      series: [{}, { stroke: STROKE, width: 1.5 }],
      hooks: {
        setCursor: [
          (u: uPlot) => {
            const idx = u.cursor.idx;
            if (idx == null || idx < 0) {
              setHover(null);
              return;
            }
            const xs = u.data[0] as number[];
            const ys = u.data[1] as number[];
            const ts = xs[idx];
            const value = ys[idx];
            if (ts == null || !Number.isFinite(value)) {
              setHover(null);
              return;
            }
            setHover({ ts, value });
          },
        ],
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
            // entry inside the visible window. Pulled from the ref, not
            // captured at memoisation time.
            const liveEvents = eventsRef.current;
            if (liveEvents && liveEvents.length > 0) {
              ctx.save();
              ctx.strokeStyle = "rgb(255 176 0 / 0.55)";
              ctx.setLineDash([]);
              ctx.lineWidth = 1;
              for (const ev of liveEvents) {
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
    [warn, crit, logY, syncKey],
  );

  // Create plot only after width is known and the series has enough
  // samples to drive a sensible time axis. Before that we render a
  // "collecting samples" placeholder; uPlot on empty data picks an
  // arbitrary X range and the operator sees years like 2026-2029 on
  // the axis.
  const dataReady = series[0].length >= 2;
  useEffect(() => {
    if (!containerRef.current) return;
    if (widthRef.current === 0) return;
    if (!dataReady) return;
    plotRef.current = new uPlot(options, series, containerRef.current);
    return () => {
      plotRef.current?.destroy();
      plotRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [options, dataReady]);

  // Live resize via setSize — no plot recreation, just a canvas redraw.
  useEffect(() => {
    if (plotRef.current && width > 0) {
      plotRef.current.setSize({ width, height: HEIGHT_PX });
    }
  }, [width]);

  useEffect(() => {
    plotRef.current?.setData(series);
  }, [series]);

  return (
    // `min-w-0` keeps the flex container from auto-expanding to fit its
    // longest child's intrinsic width — without it the footer's idle vs
    // hover text could push the wrap wider, ResizeObserver would refire,
    // options/chart would rebuild, and the operator would see the page
    // jitter on every mouse move.
    <div
      ref={wrapRef}
      className="flex min-w-0 flex-col gap-1 rounded-md border border-border bg-surface p-3"
    >
      {/*
        The label flex-shrinks (min-w-0) and the value never does (shrink-0).
        The number is always the operator's primary read; if the tile is
        narrow, the title abbreviates with an ellipsis instead of squeezing
        the number out.
      */}
      <div className="flex items-baseline justify-between gap-3">
        {tip ? (
          <InfoLabel
            tip={tip}
            className="min-w-0 text-[10px] uppercase tracking-[0.18em] text-text-dim"
          >
            <span className="truncate">{label}</span>
          </InfoLabel>
        ) : (
          <span className="min-w-0 truncate text-[10px] uppercase tracking-[0.18em] text-text-dim">
            {label}
          </span>
        )}
        <span
          className="shrink-0 whitespace-nowrap font-mono text-sm font-semibold text-text tabular"
          title={valueText}
        >
          {valueText}
        </span>
      </div>
      {series[0].length >= 2 ? (
        <div ref={containerRef} className="w-full" />
      ) : (
        <div
          style={{ height: HEIGHT_PX }}
          className="flex items-center justify-center text-[10px] text-text-dim"
        >
          collecting samples · {series[0].length}/120
        </div>
      )}
      {/*
        Fixed-height single-line footer. Idle and hover states use the
        same h/leading so swapping content cannot bump the card by a
        pixel — the trigger of the page-wide jitter the operator hits
        when sweeping the mouse across multiple sparklines on Overview.
      */}
      <div className="flex h-4 items-center justify-between gap-3 overflow-hidden whitespace-nowrap text-[10px] leading-4 text-text-dim tabular">
        {hover ? (
          <>
            <span className="truncate">{new Date(hover.ts * 1000).toLocaleTimeString()}</span>
            <span className="font-mono text-text-muted">{formatHoverValue(hover.value)}</span>
          </>
        ) : (
          <>
            <span className="truncate">{label.toLowerCase()} · last {valueText}</span>
            <span className="text-text-dim">hover for point</span>
          </>
        )}
      </div>
    </div>
  );
}
