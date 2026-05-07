import { useEffect, useRef } from "react";

interface MiniSparklineProps {
  values: number[];
  width?: number;
  height?: number;
  /** Resolved hex / rgb stroke color (no Tailwind classes — the canvas needs a literal). */
  stroke: string;
  /** Optional fixed range; otherwise auto-fit. */
  min?: number;
  max?: number;
}

/**
 * Tiny canvas sparkline meant to be inlined into a table cell. Avoids uPlot
 * because each row would otherwise spin up a dedicated chart instance — fine
 * for the four golden-signals cards but expensive across dozens of rows.
 */
export function MiniSparkline({
  values,
  width = 80,
  height = 16,
  stroke,
  min,
  max,
}: MiniSparklineProps) {
  const ref = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, width, height);
    if (values.length === 0) return;
    const lo = min ?? Math.min(...values);
    const hi = max ?? Math.max(...values);
    const span = hi - lo || 1;
    ctx.strokeStyle = stroke;
    ctx.lineWidth = 1;
    ctx.beginPath();
    values.forEach((v, i) => {
      const x = values.length === 1 ? width / 2 : (i / (values.length - 1)) * (width - 1);
      const y = height - 1 - ((v - lo) / span) * (height - 2);
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    });
    ctx.stroke();
  }, [values, width, height, stroke, min, max]);

  return <canvas ref={ref} className="block" />;
}
