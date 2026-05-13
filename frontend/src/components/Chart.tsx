import { useEffect, useRef } from "react";
import uPlot, { type AlignedData, type Options } from "uplot";
import "uplot/dist/uPlot.min.css";

interface ChartProps {
  data: AlignedData;
  options: Options;
}

/**
 * Thin wrapper around uPlot. Re-creates the chart when options change,
 * setData when data changes, and resizes via ResizeObserver so the canvas
 * tracks its container — without this the chart kept the 1024 px width
 * encoded in the per-chart options and left a dead strip on the right
 * once the page container stretched past max-w-6xl.
 */
export function Chart({ data, options }: ChartProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const plotRef = useRef<uPlot | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const startWidth = Math.max(container.clientWidth, 1);
    plotRef.current = new uPlot(
      { ...options, width: startWidth },
      data,
      container,
    );
    const ro = new ResizeObserver(([entry]) => {
      const w = Math.floor(entry.contentRect.width);
      if (w > 0 && plotRef.current) {
        plotRef.current.setSize({
          width: w,
          height: options.height ?? plotRef.current.height,
        });
      }
    });
    ro.observe(container);
    return () => {
      ro.disconnect();
      plotRef.current?.destroy();
      plotRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [options]);

  useEffect(() => {
    plotRef.current?.setData(data);
  }, [data]);

  return <div ref={containerRef} className="w-full" />;
}
