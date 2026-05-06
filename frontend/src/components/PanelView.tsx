// Fullscreen drill-down modal for any chart in the dashboard. Opens via
// `?panel=<id>` on the route (so deep-linking and browser back work);
// closes on Escape, the backdrop, or the explicit ✕ button. Operators
// asked for "Grafana view" — large canvas, percentile table over the
// visible window, cross-hair readout, event annotations from /api/events.
//
// The component takes the same `series` shape Sparkline / AreaChart use
// (`[xs, ys, ...]`) so callers do not have to reshape data; PanelView is
// content-agnostic and just picks the right mode per `kind`.

import { useEffect, useMemo, useRef, useState } from "react";
import uPlot, { type Options } from "uplot";
import "uplot/dist/uPlot.min.css";
import type { ChartEvent } from "./Sparkline";
import { summaryStats } from "../lib/quantile";

export type PanelKind = "line" | "stackedArea" | "dualAxis";

export interface PanelViewProps {
  open: boolean;
  title: string;
  kind: PanelKind;
  /// Series in uPlot shape: first entry is the time axis (seconds), rest
  /// are numeric series (one per legend label).
  data: [number[], ...number[][]];
  labels: string[];
  fills?: string[];
  /// Right-axis series indices (only for `kind = "dualAxis"`).
  rightSeries?: number[];
  rightLogScale?: boolean;
  warn?: number;
  crit?: number;
  units?: string;
  events?: ChartEvent[];
  onClose: () => void;
}

const TIME_RANGES: { label: string; ms: number }[] = [
  { label: "1m", ms: 60_000 },
  { label: "5m", ms: 5 * 60_000 },
  { label: "15m", ms: 15 * 60_000 },
  { label: "1h", ms: 60 * 60_000 },
  { label: "all", ms: 0 },
];

export function PanelView({
  open,
  title,
  kind,
  data,
  labels,
  fills,
  rightSeries,
  rightLogScale,
  warn,
  crit,
  units,
  events,
  onClose,
}: PanelViewProps) {
  // Esc closes the modal regardless of focus location.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const containerRef = useRef<HTMLDivElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const plotRef = useRef<uPlot | null>(null);
  const [width, setWidth] = useState(0);
  const [height, setHeight] = useState(0);
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const [rangeMs, setRangeMs] = useState<number>(0); // 0 = all

  // Window the data to the selected range. xs in seconds — convert ms→s.
  const windowed = useMemo<[number[], ...number[][]]>(() => {
    if (rangeMs === 0) return data;
    const xs = data[0];
    if (xs.length === 0) return data;
    const cutoff = xs[xs.length - 1] - rangeMs / 1000;
    let startIdx = xs.findIndex((t) => t >= cutoff);
    if (startIdx < 0) startIdx = 0;
    const slicedXs = xs.slice(startIdx);
    const sliced = data.slice(1).map((s) => s.slice(startIdx));
    return [slicedXs, ...sliced] as [number[], ...number[][]];
  }, [data, rangeMs]);

  // Stack for stackedArea: cumulative sum so each series paints on top of
  // the previous. Same trick as AreaChart.tsx.
  const series = useMemo<[number[], ...number[][]]>(() => {
    if (kind !== "stackedArea") return windowed;
    const xs = windowed[0];
    const cumulative: number[][] = [];
    for (let i = 1; i < windowed.length; i++) {
      const prev = cumulative[cumulative.length - 1] ?? new Array(xs.length).fill(0);
      const next = windowed[i].map((v, idx) => (prev[idx] ?? 0) + v);
      cumulative.push(next);
    }
    return [xs, ...cumulative] as [number[], ...number[][]];
  }, [windowed, kind]);

  // Resize observer — sized against the modal container.
  useEffect(() => {
    if (!open || !wrapRef.current) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0].contentRect;
      setWidth(Math.floor(r.width));
      setHeight(Math.max(220, Math.floor(r.height) - 280));
    });
    ro.observe(wrapRef.current);
    return () => ro.disconnect();
  }, [open]);

  const options: Options = useMemo(() => {
    const isDual = kind === "dualAxis";
    const isStack = kind === "stackedArea";
    const opt: Options = {
      width: width || 600,
      height: height || 300,
      cursor: { points: { size: 5 } },
      legend: { show: false },
      scales: isDual
        ? {
            y: { auto: true },
            y2: rightLogScale ? { distr: 3 } : { auto: true },
          }
        : { y: { auto: true } },
      axes: [
        { stroke: "rgb(154 148 133)", grid: { stroke: "rgb(31 31 31)" } },
        {
          stroke: "rgb(154 148 133)",
          grid: { stroke: "rgb(31 31 31)" },
          scale: "y",
        },
        ...(isDual
          ? [
              {
                stroke: "rgb(154 148 133)",
                grid: { show: false },
                scale: "y2",
                side: 1,
              },
            ]
          : []),
      ],
      series: [
        {},
        ...labels.map((label, i) => {
          const stroke = fills?.[i] ?? "rgb(255 176 0)";
          const onRight = rightSeries?.includes(i + 1);
          return {
            label,
            stroke,
            fill: isStack ? stroke : undefined,
            width: 1.5,
            scale: onRight ? "y2" : "y",
          };
        }),
      ],
      hooks: {
        setCursor: [
          (u: uPlot) => {
            const idx = u.cursor.idx;
            if (idx == null || idx < 0) {
              setHoverIdx(null);
              return;
            }
            setHoverIdx(idx);
          },
        ],
        draw: [
          (u: uPlot) => {
            const ctx = u.ctx;
            // Threshold lines (only for line/single-axis kinds where warn/crit
            // map to the y axis cleanly).
            if (warn !== undefined && kind === "line") {
              const yPx = u.valToPos(warn, "y", true);
              if (Number.isFinite(yPx)) {
                ctx.save();
                ctx.strokeStyle = "rgb(255 176 0 / 0.6)";
                ctx.setLineDash([4, 4]);
                ctx.lineWidth = 1;
                ctx.beginPath();
                ctx.moveTo(u.bbox.left, yPx);
                ctx.lineTo(u.bbox.left + u.bbox.width, yPx);
                ctx.stroke();
                ctx.restore();
              }
            }
            if (crit !== undefined && kind === "line") {
              const yPx = u.valToPos(crit, "y", true);
              if (Number.isFinite(yPx)) {
                ctx.save();
                ctx.strokeStyle = "rgb(255 77 77 / 0.6)";
                ctx.setLineDash([4, 4]);
                ctx.lineWidth = 1;
                ctx.beginPath();
                ctx.moveTo(u.bbox.left, yPx);
                ctx.lineTo(u.bbox.left + u.bbox.width, yPx);
                ctx.stroke();
                ctx.restore();
              }
            }
            // Event vertical lines.
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
    };
    return opt;
  }, [width, height, kind, labels, fills, rightSeries, rightLogScale, warn, crit, events]);

  // (Re)create the plot on width/height/options change.
  useEffect(() => {
    if (!open) return;
    if (!containerRef.current) return;
    if (width === 0 || height === 0) return;
    plotRef.current = new uPlot(options, series, containerRef.current);
    return () => {
      plotRef.current?.destroy();
      plotRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, options, width, height]);

  useEffect(() => {
    plotRef.current?.setData(series);
  }, [series]);

  // Compute summary stats per series over the visible window.
  const summaries = useMemo(() => {
    return labels.map((_, i) => summaryStats(windowed[i + 1] ?? []));
  }, [labels, windowed]);

  if (!open) return null;

  const hoverTs = hoverIdx !== null ? series[0][hoverIdx] : null;
  const hoverValues =
    hoverIdx !== null
      ? labels.map((_, i) => {
          if (kind === "stackedArea") {
            const cur = series[i + 1][hoverIdx];
            const prev = i === 0 ? 0 : series[i][hoverIdx];
            return cur - prev;
          }
          return series[i + 1][hoverIdx];
        })
      : null;

  const fmt = (v: number | null) => {
    if (v === null) return "—";
    if (Math.abs(v) >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
    if (Math.abs(v) >= 10_000) return `${(v / 1000).toFixed(0)}k`;
    if (Math.abs(v) >= 1000) return `${(v / 1000).toFixed(1)}k`;
    if (Math.abs(v) >= 10) return v.toFixed(0);
    return v.toFixed(2);
  };

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-bg/85 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        ref={wrapRef}
        className="m-6 flex h-[90vh] w-[min(96vw,1400px)] flex-col border border-border bg-surface"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex items-center justify-between border-b border-border px-4 py-3">
          <div>
            <div className="text-[10px] uppercase tracking-[0.2em] text-text-dim">panel</div>
            <h2 className="font-mono text-base font-semibold text-text">{title}</h2>
          </div>
          <div className="flex items-center gap-2">
            <div className="flex items-center gap-1 border border-border-strong px-1 py-0.5">
              {TIME_RANGES.map((r) => (
                <button
                  key={r.label}
                  type="button"
                  onClick={() => setRangeMs(r.ms)}
                  className={`px-2 py-0.5 text-xs font-mono uppercase tracking-wider ${
                    rangeMs === r.ms
                      ? "bg-accent/20 text-accent"
                      : "text-text-muted hover:text-text"
                  }`}
                >
                  {r.label}
                </button>
              ))}
            </div>
            <button
              type="button"
              onClick={onClose}
              className="border border-border-strong px-2 py-0.5 text-xs font-mono uppercase tracking-wider text-text-muted hover:text-accent"
              title="Close (Esc)"
            >
              ✕
            </button>
          </div>
        </header>

        <div ref={containerRef} className="flex-1 px-4 py-3" />

        <div className="border-t border-border bg-surface-2 px-4 py-2 font-mono text-xs">
          {hoverTs !== null && hoverValues ? (
            <div className="flex flex-wrap gap-x-6 text-text">
              <span>{new Date(hoverTs * 1000).toLocaleTimeString()}</span>
              {labels.map((l, i) => (
                <span key={l} style={{ color: fills?.[i] }}>
                  {l} <span className="text-text">{fmt(hoverValues[i])}</span>
                  {units ? ` ${units}` : ""}
                </span>
              ))}
            </div>
          ) : (
            <span className="text-text-dim">hover for value at cursor</span>
          )}
        </div>

        <div className="border-t border-border px-4 py-3">
          <div className="mb-1 text-[10px] uppercase tracking-[0.2em] text-text-dim">
            summary over the visible window
          </div>
          <table className="w-full text-xs tabular">
            <thead className="text-text-muted">
              <tr>
                <th className="text-left">series</th>
                <th className="text-right">count</th>
                <th className="text-right">min</th>
                <th className="text-right">avg</th>
                <th className="text-right">p50</th>
                <th className="text-right">p95</th>
                <th className="text-right">p99</th>
                <th className="text-right">max</th>
              </tr>
            </thead>
            <tbody>
              {labels.map((l, i) => {
                const s = summaries[i];
                return (
                  <tr key={l} className="border-t border-border/50">
                    <td className="py-1" style={{ color: fills?.[i] }}>
                      ● {l}
                    </td>
                    <td className="text-right">{s.count}</td>
                    <td className="text-right">{fmt(s.min)}</td>
                    <td className="text-right">{fmt(s.avg)}</td>
                    <td className="text-right">{fmt(s.p50)}</td>
                    <td className="text-right">{fmt(s.p95)}</td>
                    <td className="text-right">{fmt(s.p99)}</td>
                    <td className="text-right">{fmt(s.max)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
