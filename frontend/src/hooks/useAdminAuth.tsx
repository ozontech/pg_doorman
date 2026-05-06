import { createContext, useCallback, useContext, useState, type ReactNode } from "react";

interface Credentials {
  username: string;
  password: string;
}

interface AdminAuthValue {
  creds: Credentials | null;
  setCreds: (next: Credentials | null) => void;
  authHeader: () => Record<string, string>;
}

const AdminAuthContext = createContext<AdminAuthValue | null>(null);

export function AdminAuthProvider({ children }: { children: ReactNode }) {
  const [creds, setCreds] = useState<Credentials | null>(null);

  const authHeader = useCallback((): Record<string, string> => {
    if (!creds) return {};
    const token = btoa(`${creds.username}:${creds.password}`);
    return { Authorization: `Basic ${token}` };
  }, [creds]);

  return (
    <AdminAuthContext.Provider value={{ creds, setCreds, authHeader }}>
      {children}
    </AdminAuthContext.Provider>
  );
}

export function useAdminAuth(): AdminAuthValue {
  const ctx = useContext(AdminAuthContext);
  if (!ctx) throw new Error("useAdminAuth must be used inside AdminAuthProvider");
  return ctx;
}
