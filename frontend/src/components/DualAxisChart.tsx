import { useMemo } from "react";
import type { Options } from "uplot";
import type uPlot from "uplot";
import { Chart } from "./Chart";

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
}: DualAxisChartProps) {
  const options: Options = useMemo(
    () => ({
      width: 1024,
      height,
      cursor: syncKey ? { sync: { key: syncKey } } : undefined,
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
        draw: [
          (u: uPlot) => {
            if (rightWarn === undefined && rightCrit === undefined) return;
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
    ],
  );

  return (
    <div className="w-full overflow-hidden">
      <Chart data={data} options={options} />
    </div>
  );
}
