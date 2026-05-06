import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { setOnUnauthorized } from "../api";

interface Credentials {
  username: string;
  password: string;
}

interface AdminAuthValue {
  creds: Credentials | null;
  setCreds: (next: Credentials | null, remember?: boolean) => void;
  authHeader: () => Record<string, string>;
  /** Bumps every time api.ts saw a 401. AuthGate watches this. */
  unauthorizedAt: number | null;
  clearUnauthorized: () => void;
  /** True when credentials were loaded from localStorage on mount, or saved
   * via `setCreds(_, remember=true)`. The AuthGate checkbox reflects this. */
  remembered: boolean;
}

const AdminAuthContext = createContext<AdminAuthValue | null>(null);

const STORAGE_KEY = "pgdoorman.admin-auth";

function loadStored(): Credentials | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (
      parsed &&
      typeof parsed === "object" &&
      "username" in parsed &&
      "password" in parsed &&
      typeof (parsed as Credentials).username === "string" &&
      typeof (parsed as Credentials).password === "string"
    ) {
      return parsed as Credentials;
    }
    return null;
  } catch {
    return null;
  }
}

export function AdminAuthProvider({ children }: { children: ReactNode }) {
  const initial = loadStored();
  const [creds, setCredsState] = useState<Credentials | null>(initial);
  const [remembered, setRemembered] = useState<boolean>(initial !== null);
  const [unauthorizedAt, setUnauthorizedAt] = useState<number | null>(null);

  useEffect(() => {
    setOnUnauthorized(() => setUnauthorizedAt(Date.now()));
    return () => setOnUnauthorized(() => {});
  }, []);

  const setCreds = useCallback((next: Credentials | null, remember = false) => {
    setCredsState(next);
    setRemembered(remember && next !== null);
    try {
      if (next && remember) {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      } else {
        // Either user cleared creds, or chose "do not remember" — wipe any
        // earlier persisted copy so a shared workstation does not leak.
        localStorage.removeItem(STORAGE_KEY);
      }
    } catch {
      /* private mode / quota / disabled — non-fatal */
    }
  }, []);

  const authHeader = useCallback((): Record<string, string> => {
    if (!creds) return {};
    const token = btoa(`${creds.username}:${creds.password}`);
    return { Authorization: `Basic ${token}` };
  }, [creds]);

  const clearUnauthorized = useCallback(() => setUnauthorizedAt(null), []);

  return (
    <AdminAuthContext.Provider
      value={{
        creds,
        setCreds,
        authHeader,
        unauthorizedAt,
        clearUnauthorized,
        remembered,
      }}
    >
      {children}
    </AdminAuthContext.Provider>
  );
}

export function useAdminAuth(): AdminAuthValue {
  const ctx = useContext(AdminAuthContext);
  if (!ctx) throw new Error("useAdminAuth must be used inside AdminAuthProvider");
  return ctx;
}
