import { useEffect, useRef, useState } from "react";

interface PollState<T> {
  data: T | null;
  error: Error | null;
  lastUpdated: number | null;
}

/**
 * Calls fetcher on mount and every intervalMs ms. Cancels the in-flight
 * request via AbortController on unmount and on dependency change. Phase 5
 * does not call this hook from any page; it is here so phase 6 has the
 * primitive ready.
 */
export function usePoll<T>(
  fetcher: (signal: AbortSignal) => Promise<T>,
  intervalMs = 1500,
): PollState<T> {
  const [state, setState] = useState<PollState<T>>({
    data: null,
    error: null,
    lastUpdated: null,
  });
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;

  useEffect(() => {
    let cancelled = false;
    const controller = new AbortController();
    const tick = () => {
      fetcherRef
        .current(controller.signal)
        .then((data) => {
          if (cancelled) return;
          setState({ data, error: null, lastUpdated: Date.now() });
        })
        .catch((e: unknown) => {
          if (cancelled) return;
          if (e instanceof DOMException && e.name === "AbortError") return;
          setState((prev) => ({
            ...prev,
            error: e instanceof Error ? e : new Error(String(e)),
          }));
        });
    };
    tick();
    const id = window.setInterval(tick, intervalMs);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearInterval(id);
    };
  }, [intervalMs]);

  return state;
}
