import { useMemo } from "react";
import type uPlot from "uplot";
import type { Options } from "uplot";
import { Chart } from "./Chart";

interface SparklineProps {
  label: string;
  valueText: string;
  series: [number[], number[]];
  warn?: number;
  crit?: number;
  logY?: boolean;
  syncKey?: string;
}

const HEIGHT_PX = 80;
const STROKE = "rgb(34 184 207)";
const WARN_STROKE = "rgb(245 165 36 / 0.6)";
const CRIT_STROKE = "rgb(229 72 77 / 0.6)";

export function Sparkline({
  label,
  valueText,
  series,
  warn,
  crit,
  logY,
  syncKey,
}: SparklineProps) {
  const options: Options = useMemo(
    () => ({
      width: 200,
      height: HEIGHT_PX,
      cursor: syncKey ? { sync: { key: syncKey } } : undefined,
      legend: { show: false },
      scales: {
        y: logY ? { distr: 3 } : { auto: true },
      },
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
    [warn, crit, logY, syncKey],
  );

  return (
    <div className="flex flex-col gap-1 px-3 py-3 border-r border-border last:border-r-0">
      <div className="flex items-baseline justify-between">
        <span className="text-xs text-text-muted uppercase tracking-wide">{label}</span>
        <span className="text-lg font-semibold font-mono text-text tabular">{valueText}</span>
      </div>
      <Chart data={series} options={options} />
    </div>
  );
}
