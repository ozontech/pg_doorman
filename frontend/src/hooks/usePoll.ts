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
    let intervalId: number | null = null;
    // Sequence id stamps every dispatched request. A response is committed
    // only when its stamp matches `seq` at completion time; older in-flight
    // responses lose the race and are discarded. Without this, a slow
    // /api/logs or /api/clients can land after a fresher one and overwrite
    // the UI with stale rows (and, for /api/logs, double-emit entries by
    // re-using the `since` cursor).
    let seq = 0;
    let inflight = 0;

    const tick = () => {
      // Skip background ticks: when the tab is hidden, browsers throttle
      // setInterval (often to 1 Hz minimum) and abort/clear pending fetches
      // anyway. A skipped tick keeps the user-visible last sample fresh
      // without faking new history points.
      if (typeof document !== "undefined" && document.hidden) return;
      // Drop ticks while a request is in flight. The visible interval is
      // intervalMs from the last completion, not from the last dispatch —
      // matters for slow endpoints (/api/logs at 1.5 s with a 600 ms
      // round-trip skipped one out of every two ticks otherwise).
      if (inflight > 0) return;
      seq += 1;
      const mySeq = seq;
      inflight += 1;
      fetcherRef
        .current(controller.signal)
        .then((data) => {
          if (cancelled || mySeq !== seq) return;
          setState({ data, error: null, lastUpdated: Date.now() });
        })
        .catch((e: unknown) => {
          if (cancelled || mySeq !== seq) return;
          if (e instanceof DOMException && e.name === "AbortError") return;
          setState((prev) => ({
            ...prev,
            error: e instanceof Error ? e : new Error(String(e)),
          }));
        })
        .finally(() => {
          inflight -= 1;
        });
    };

    const startInterval = () => {
      if (intervalId !== null) return;
      intervalId = window.setInterval(tick, intervalMs);
    };
    const stopInterval = () => {
      if (intervalId === null) return;
      window.clearInterval(intervalId);
      intervalId = null;
    };
    const onVisibility = () => {
      if (document.hidden) {
        stopInterval();
      } else {
        // Resume immediately so the user does not wait a full interval
        // for the first sample after returning to the tab.
        tick();
        startInterval();
      }
    };

    tick();
    if (typeof document === "undefined" || !document.hidden) startInterval();
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", onVisibility);
    }

    return () => {
      cancelled = true;
      controller.abort();
      stopInterval();
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", onVisibility);
      }
    };
  }, [intervalMs]);

  return state;
}
