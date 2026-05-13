import { useEffect, useState } from "react";

const DEFAULT_MAX_POINTS = 120; // 120 × 1.5 s polling = 3 min window per parent spec §10.2.

export interface HistoryHandle<T> {
  history: T[];
  push: (value: T) => void;
  /// Replace the rolling window with `next`. Used to clear the buffer when a
  /// stale-tab gap is detected so the chart does not bridge it with a flat
  /// line.
  replace: (next: T[]) => void;
}

/**
 * Rolling window of the latest `maxPoints` values keyed by `key`. Persisted
 * in localStorage scoped by `window.location.host` so two tabs pointing at
 * different poolers (pooler-a and pooler-b) keep separate histories.
 * sessionStorage was scoped to a single tab, which made charts blink to
 * empty whenever an operator reopened the console; localStorage preserves
 * the rolling window across reloads and across tabs viewing the same
 * pooler. Storage write failures (private mode, quota) are silent — the
 * in-memory history still works.
 */
export function useHistory<T>(key: string, maxPoints = DEFAULT_MAX_POINTS): HistoryHandle<T> {
  const origin = typeof window !== "undefined" ? window.location.host : "any";
  const storageKey = `pgdoorman.history.${origin}.${key}`;
  const [history, setHistory] = useState<T[]>(() => {
    try {
      const raw = localStorage.getItem(storageKey);
      if (!raw) return [];
      const parsed: unknown = JSON.parse(raw);
      return Array.isArray(parsed) ? (parsed as T[]) : [];
    } catch {
      return [];
    }
  });

  useEffect(() => {
    try {
      localStorage.setItem(storageKey, JSON.stringify(history));
    } catch {
      /* storage quota or private mode — no-op. */
    }
  }, [history, storageKey]);

  const push = (value: T) => {
    setHistory((prev) => {
      const next =
        prev.length >= maxPoints ? prev.slice(prev.length - maxPoints + 1) : prev.slice();
      next.push(value);
      return next;
    });
  };

  const replace = (next: T[]) => {
    setHistory(next.slice(0, maxPoints));
  };

  return { history, push, replace };
}
