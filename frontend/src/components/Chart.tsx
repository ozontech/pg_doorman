import { useEffect, useRef } from "react";
import uPlot, { type AlignedData, type Options } from "uplot";
import "uplot/dist/uPlot.min.css";

interface ChartProps {
  data: AlignedData;
  options: Options;
}

/**
 * Thin wrapper around uPlot. Re-creates the chart when options change,
 * setData when data changes. The cross-hair sync key, if any, lives on
 * options.cursor.sync — caller controls the group name (we use "overview"
 * for all phase 6a charts so hover tracks across the strip).
 */
export function Chart({ data, options }: ChartProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const plotRef = useRef<uPlot | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    plotRef.current = new uPlot(options, data, containerRef.current);
    return () => {
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
