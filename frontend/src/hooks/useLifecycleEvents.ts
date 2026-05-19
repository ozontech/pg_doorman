import { useEffect, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { apiGet } from "../api";
import { useAdminAuth } from "./useAdminAuth";
import type { EventEntryDto, EventsDto } from "../types";

/**
 * Incremental poll of `/api/events`, surfacing high-severity entries as
 * toasts. On first mount we record the current `next_seq` without
 * notifying — opening a fresh tab during an incident otherwise replays
 * the entire ring as a stream of toasts.
 *
 * The events ring (`src/admin/events.rs`) is also the source for the
 * timeline annotations on Overview/Wall, so this hook does not own the
 * data — it only decides which entries warrant an interruption.
 */

const POLL_MS = 3_000;
const MAX_FETCH = 50;

export function useLifecycleEvents() {
  const { authHeader } = useAdminAuth();
  // Two-state baseline: `null` = the very first poll has not landed yet,
  // we cannot tell what is new vs. retained, so suppress everything.
  // `number` = we have a known cursor, anything strictly above it is
  // a fresh event to react to.
  const sinceRef = useRef<number | null>(null);

  const { data } = useQuery({
    queryKey: ["lifecycle-events", authHeader],
    queryFn: ({ signal }) =>
      apiGet<EventsDto>(
        `/api/events?since=${sinceRef.current ?? 0}&max=${MAX_FETCH}`,
        authHeader,
        signal,
      ),
    refetchInterval: POLL_MS,
  });

  useEffect(() => {
    if (!data) return;
    const baseline = sinceRef.current;
    if (baseline === null) {
      // First successful poll — adopt next_seq as the baseline without
      // toasting on any of the events that arrived with it.
      sinceRef.current = data.next_seq;
      return;
    }
    for (const ev of data.events) {
      if (ev.seq <= baseline) continue;
      notify(ev);
    }
    sinceRef.current = Math.max(baseline, data.next_seq);
  }, [data]);
}

function notify(ev: EventEntryDto) {
  switch (ev.target) {
    case "CONFIG_VALIDATION_ERROR":
      // Operator-actionable: a deploy step (SIGHUP, admin RELOAD,
      // /api/admin/reload) left the running config unchanged because
      // the new config did not validate. Keep the toast longer than
      // the default so the operator notices it after stepping away
      // from the dashboard.
      toast.error(ev.message, { duration: 10_000 });
      break;
    // PROCESS_START, RELOAD, PAUSE, RESUME, RECONNECT are timeline-only
    // — they already render as annotations on Overview/Wall; toasting
    // every one of them would pile up during normal operator activity.
    default:
      break;
  }
}
