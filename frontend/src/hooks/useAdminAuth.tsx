import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { setOnForbidden, setOnUnauthorized } from "../api";
import {
  clearSsoToken,
  getValidSsoToken,
  SSO_TOKEN_KEY,
} from "../lib/jwt";
import type { Role } from "../types";

interface BasicCreds {
  username: string;
  password: string;
}

interface AdminAuthValue {
  /** Basic-auth credentials, when the operator has signed in via the form. */
  basic: BasicCreds | null;
  setBasic: (next: BasicCreds | null, remember?: boolean) => void;
  /** Last known SSO JWT, when present and unexpired. */
  ssoToken: string | null;
  setSsoToken: (next: string | null) => void;
  /** Last role we learned from `/api/auth/config`. */
  role: Role;
  setRole: (next: Role) => void;
  /** Compose the right `Authorization` header for the next request. */
  authHeader: () => Record<string, string>;
  /** Bumps every time api.ts saw a 401. AuthGate watches this. */
  unauthorizedAt: number | null;
  /** Bumps every time api.ts saw a 403. UI banners watch this. */
  forbiddenAt: number | null;
  clearTransients: () => void;
  /** True when Basic creds were loaded from localStorage on mount, or saved
   * via `setBasic(_, remember=true)`. The AuthGate checkbox reflects this. */
  remembered: boolean;
}

const AdminAuthContext = createContext<AdminAuthValue | null>(null);

const STORAGE_KEY = "pgdoorman.admin-auth";

function loadStored(): BasicCreds | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (
      parsed &&
      typeof parsed === "object" &&
      "username" in parsed &&
      "password" in parsed &&
      typeof (parsed as BasicCreds).username === "string" &&
      typeof (parsed as BasicCreds).password === "string"
    ) {
      return parsed as BasicCreds;
    }
    return null;
  } catch {
    return null;
  }
}

export function AdminAuthProvider({ children }: { children: ReactNode }) {
  const initial = loadStored();
  const [basic, setBasicState] = useState<BasicCreds | null>(initial);
  const [remembered, setRemembered] = useState<boolean>(initial !== null);
  const [ssoToken, setSsoTokenState] = useState<string | null>(() =>
    getValidSsoToken(),
  );
  const [role, setRole] = useState<Role>("anonymous");
  const [unauthorizedAt, setUnauthorizedAt] = useState<number | null>(null);
  const [forbiddenAt, setForbiddenAt] = useState<number | null>(null);

  useEffect(() => {
    setOnUnauthorized(() => setUnauthorizedAt(Date.now()));
    setOnForbidden(() => setForbiddenAt(Date.now()));
    return () => {
      setOnUnauthorized(() => {});
      setOnForbidden(() => {});
    };
  }, []);

  const setBasic = useCallback((next: BasicCreds | null, remember = false) => {
    setBasicState(next);
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

  const setSsoToken = useCallback((next: string | null) => {
    if (next) {
      try {
        localStorage.setItem(SSO_TOKEN_KEY, next);
      } catch {
        /* non-fatal */
      }
    } else {
      clearSsoToken();
    }
    setSsoTokenState(next);
  }, []);

  const authHeader = useCallback((): Record<string, string> => {
    // Basic outranks SSO: a known admin password is the strongest
    // credential the operator can present and the backend's `classify`
    // honours the same precedence.
    if (basic) {
      const token = btoa(`${basic.username}:${basic.password}`);
      return { Authorization: `Basic ${token}` };
    }
    const sso = getValidSsoToken();
    if (sso !== ssoToken) {
      // Reconcile state with localStorage in case another tab cleared it.
      setSsoTokenState(sso);
    }
    if (sso) return { Authorization: `Bearer ${sso}` };
    return {};
  }, [basic, ssoToken]);

  const clearTransients = useCallback(() => {
    setUnauthorizedAt(null);
    setForbiddenAt(null);
  }, []);

  return (
    <AdminAuthContext.Provider
      value={{
        basic,
        setBasic,
        ssoToken,
        setSsoToken,
        role,
        setRole,
        authHeader,
        unauthorizedAt,
        forbiddenAt,
        clearTransients,
        remembered,
      }}
    >
      {children}
    </AdminAuthContext.Provider>
  );
}

export function useAdminAuth(): AdminAuthValue {
  const ctx = useContext(AdminAuthContext);
  if (!ctx)
    throw new Error("useAdminAuth must be used inside AdminAuthProvider");
  return ctx;
}
