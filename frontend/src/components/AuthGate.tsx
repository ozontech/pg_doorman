import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { apiGet, Unauthorized } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import type { VersionDto } from "../types";

/**
 * Probes /api/version on mount and on credential change. If the probe returns
 * 401, renders a basic-auth modal that locks the rest of the app until the
 * user submits credentials that satisfy /api/version. Once authorised (or
 * if /api/version is anonymously accessible), renders children.
 *
 * Phase 5 only owns the version probe. Phase 6 pages drive the same auth
 * flow indirectly: every apiGet call rethrows Unauthorized; the gate shows
 * the modal until a successful retry.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const { creds, setCreds, authHeader } = useAdminAuth();
  const [needsAuth, setNeedsAuth] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [probing, setProbing] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setProbing(true);
    setError(null);
    apiGet<VersionDto>("/api/version", authHeader)
      .then(() => {
        if (cancelled) return;
        setNeedsAuth(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        if (e instanceof Unauthorized) {
          setNeedsAuth(true);
        } else {
          setError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!cancelled) setProbing(false);
      });
    return () => {
      cancelled = true;
    };
  }, [authHeader]);

  if (probing) {
    return <div className="p-4 text-text-muted">connecting…</div>;
  }
  if (error) {
    return <div className="p-4 text-danger">{error}</div>;
  }
  if (needsAuth) {
    return <AuthModal currentCreds={creds} onSubmit={setCreds} />;
  }
  return <>{children}</>;
}

function AuthModal({
  currentCreds,
  onSubmit,
}: {
  currentCreds: { username: string; password: string } | null;
  onSubmit: (next: { username: string; password: string }) => void;
}) {
  const [username, setUsername] = useState(currentCreds?.username ?? "");
  const [password, setPassword] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit({ username, password });
  };
  return (
    <div className="fixed inset-0 flex items-center justify-center bg-bg/80 backdrop-blur-sm">
      <form
        onSubmit={submit}
        className="w-80 rounded border border-border bg-surface p-6 shadow-xl"
      >
        <h2 className="mb-4 text-md font-semibold">Sign in</h2>
        <p className="mb-4 text-sm text-text-muted">
          {currentCreds
            ? "Credentials were rejected. Try again."
            : "This pg_doorman requires admin credentials."}
        </p>
        <label className="mb-2 block text-xs uppercase tracking-wide text-text-muted">
          Username
        </label>
        <input
          autoFocus
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          className="mb-3 w-full rounded border border-border-strong bg-surface-2 px-2 py-1.5 text-sm text-text"
        />
        <label className="mb-2 block text-xs uppercase tracking-wide text-text-muted">
          Password
        </label>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          className="mb-4 w-full rounded border border-border-strong bg-surface-2 px-2 py-1.5 text-sm text-text"
        />
        <button
          type="submit"
          className="w-full rounded bg-accent px-3 py-1.5 text-sm font-medium text-accent-fg hover:bg-accent-hover"
        >
          Sign in
        </button>
      </form>
    </div>
  );
}
