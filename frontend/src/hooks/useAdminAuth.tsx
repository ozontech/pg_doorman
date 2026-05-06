import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { setOnUnauthorized } from "../api";

interface Credentials {
  username: string;
  password: string;
}

interface AdminAuthValue {
  creds: Credentials | null;
  setCreds: (next: Credentials | null) => void;
  authHeader: () => Record<string, string>;
  /** Bumps every time api.ts saw a 401. AuthGate watches this. */
  unauthorizedAt: number | null;
  clearUnauthorized: () => void;
}

const AdminAuthContext = createContext<AdminAuthValue | null>(null);

export function AdminAuthProvider({ children }: { children: ReactNode }) {
  const [creds, setCreds] = useState<Credentials | null>(null);
  const [unauthorizedAt, setUnauthorizedAt] = useState<number | null>(null);

  useEffect(() => {
    setOnUnauthorized(() => setUnauthorizedAt(Date.now()));
    return () => setOnUnauthorized(() => {});
  }, []);

  const authHeader = useCallback((): Record<string, string> => {
    if (!creds) return {};
    const token = btoa(`${creds.username}:${creds.password}`);
    return { Authorization: `Basic ${token}` };
  }, [creds]);

  const clearUnauthorized = useCallback(() => setUnauthorizedAt(null), []);

  return (
    <AdminAuthContext.Provider
      value={{ creds, setCreds, authHeader, unauthorizedAt, clearUnauthorized }}
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
