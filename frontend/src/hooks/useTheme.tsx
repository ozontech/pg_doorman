import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type ThemePref = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "pgdoorman.theme";

interface ThemeContextValue {
  /** Operator preference: explicit choice or "system" (match OS). */
  pref: ThemePref;
  /** Actual mode the page is rendering in — what "system" resolves to. */
  resolved: ResolvedTheme;
  setPref: (next: ThemePref) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function readPref(): ThemePref {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === "light" || raw === "dark" || raw === "system") return raw;
  } catch {
    /* private mode — fall through. */
  }
  return "system";
}

function resolve(pref: ThemePref): ResolvedTheme {
  if (pref === "system") {
    if (typeof window !== "undefined" && window.matchMedia) {
      return window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";
    }
    return "light";
  }
  return pref;
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [pref, setPrefState] = useState<ThemePref>(readPref);
  const [resolved, setResolved] = useState<ResolvedTheme>(() => resolve(readPref()));

  const setPref = useCallback((next: ThemePref) => {
    setPrefState(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      /* private mode — no-op. */
    }
  }, []);

  useEffect(() => {
    setResolved(resolve(pref));
  }, [pref]);

  // Track OS preference changes while pref is "system".
  useEffect(() => {
    if (pref !== "system") return;
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setResolved(mq.matches ? "dark" : "light");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [pref]);

  // Apply / unapply the .dark class on <html>. useLayoutEffect runs
  // before the browser commits the first paint, so an operator who
  // persisted "dark" does not see a flash of light theme before the
  // class lands.
  useLayoutEffect(() => {
    const root = document.documentElement;
    if (resolved === "dark") root.classList.add("dark");
    else root.classList.remove("dark");
  }, [resolved]);

  const value = useMemo<ThemeContextValue>(
    () => ({ pref, resolved, setPref }),
    [pref, resolved, setPref],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be called inside ThemeProvider");
  return ctx;
}
