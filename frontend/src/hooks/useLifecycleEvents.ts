import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../api";
import { useAdminAuth } from "./useAdminAuth";
import type { EventEntryDto, EventsDto } from "../types";

/**
 * Incremental poll of `/api/events`. Tracks the most recent
 * CONFIG_VALIDATION_ERROR so the LifecycleBanner can surface it as a
 * persistent strip until a successful RELOAD clears it — toasts are too
 * easy to miss when the operator alt-tabs to a terminal to investigate.
 *
 * The events ring (`src/admin/events.rs`) is also the source for the
 * timeline annotations on Overview/Wall, so this hook does not own the
 * data — it only decides which entries warrant an interruption.
 */

const POLL_MS = 3_000;
const MAX_FETCH = 50;

export interface ValidationErrorState {
  /** Banner text — full event message, as pushed by the backend. */
  message: string;
  /** Wall-clock ms of the rejection, for relative-time rendering. */
  ts_ms: number;
  /** seq of the event, used to dedupe repeats of the same rejection. */
  seq: number;
}

/** Last unresolved CONFIG_VALIDATION_ERROR, or null when cleared. */
let validationListeners: Array<(s: ValidationErrorState | null) => void> = [];
let currentValidationError: ValidationErrorState | null = null;

function emitValidationError(next: ValidationErrorState | null) {
  currentValidationError = next;
  for (const fn of validationListeners) fn(next);
}

/**
 * React hook for components that want to render the current validation
 * error (LifecycleBanner). Returns the last unresolved CONFIG_VALIDATION_ERROR
 * or `null`. Updates when useLifecycleEvents observes a new event.
 */
export function useValidationErrorState(): ValidationErrorState | null {
  const [state, setState] = useState<ValidationErrorState | null>(
    currentValidationError,
  );
  useEffect(() => {
    const fn = (s: ValidationErrorState | null) => setState(s);
    validationListeners.push(fn);
    return () => {
      validationListeners = validationListeners.filter((x) => x !== fn);
    };
  }, []);
  return state;
}

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
      // notifying on any event that arrived with it. The banner *does*
      // pick up a still-open CONFIG_VALIDATION_ERROR from the initial
      // batch though, so an operator opening the UI after the failure
      // still sees the message.
      sinceRef.current = data.next_seq;
      seedValidationError(data.events);
      return;
    }
    for (const ev of data.events) {
      if (ev.seq <= baseline) continue;
      observe(ev);
    }
    sinceRef.current = Math.max(baseline, data.next_seq);
  }, [data]);
}

/**
 * On the very first poll: scan the batch for the latest still-open
 * CONFIG_VALIDATION_ERROR (no successful RELOAD after it). That is the
 * one the banner should keep visible — an operator who joined late
 * still needs to know the deploy step rejected.
 */
function seedValidationError(events: EventEntryDto[]) {
  let latestError: ValidationErrorState | null = null;
  for (const ev of events) {
    if (ev.target === "CONFIG_VALIDATION_ERROR") {
      latestError = { message: ev.message, ts_ms: ev.ts_ms, seq: ev.seq };
    } else if (ev.target === "RELOAD") {
      latestError = null;
    }
  }
  if (latestError) emitValidationError(latestError);
}

function observe(ev: EventEntryDto) {
  if (ev.target === "CONFIG_VALIDATION_ERROR") {
    emitValidationError({
      message: ev.message,
      ts_ms: ev.ts_ms,
      seq: ev.seq,
    });
  } else if (ev.target === "RELOAD") {
    // A successful RELOAD landed after the validation failure — the
    // operator's next edit took effect, banner should clear.
    if (currentValidationError) emitValidationError(null);
  }
}
