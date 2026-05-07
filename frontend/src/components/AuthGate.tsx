import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { fetchAuthConfig, Forbidden, Unauthorized } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import {
  captureTokenFromUrl,
  redirectToSso,
  startTokenRefresh,
} from "../lib/sso";
import { getValidSsoToken } from "../lib/jwt";
import type { AuthConfig } from "../types";

/**
 * Probes /api/auth/config on mount and after every credential change.
 * The endpoint is anonymous, so the response always tells us whether
 * SSO is wired and, when this request did carry valid credentials,
 * what role the user has.
 *
 * We render children when current_user is non-null (any role above
 * Anonymous), or when the path the user navigated to is reachable as
 * Anonymous (the role check happens per-request on the backend; the
 * gate is just a UX shortcut to not show stale 401 toasts).
 *
 * 401 from a downstream API call re-arms the sign-in modal. 403 does
 * NOT — credentials are valid, the role is too low. The UI shows a
 * forbidden banner instead so the operator understands they need
 * Admin/Basic to perform that specific action.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const {
    basic,
    setBasic,
    setSsoToken,
    setRole,
    authHeader,
    unauthorizedAt,
    forbiddenAt,
    clearTransients,
    remembered,
  } = useAdminAuth();

  const [authConfig, setAuthConfig] = useState<AuthConfig | null>(null);
  const [needsAuth, setNeedsAuth] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [probing, setProbing] = useState(true);

  // Capture `?token=` returned by the SSO proxy on first load. The
  // captureTokenFromUrl helper rewrites the URL clean of the param so
  // the rest of the SPA does not see it.
  useEffect(() => {
    if (captureTokenFromUrl()) {
      setSsoToken(getValidSsoToken());
    }
  }, [setSsoToken]);

  // Probe /api/auth/config on mount, on Basic credentials change, and
  // any time api.ts saw a 401 elsewhere.
  useEffect(() => {
    let cancelled = false;
    setProbing(true);
    setError(null);
    fetchAuthConfig(authHeader)
      .then((cfg) => {
        if (cancelled) return;
        setAuthConfig(cfg);
        const role = cfg.current_user?.role ?? "anonymous";
        setRole(role);
        // Need explicit auth only when the path requires Sso/Admin and
        // the response did not carry an identity. The backend itself
        // gates per-request; here we just decide whether to surface the
        // sign-in modal.
        setNeedsAuth(cfg.current_user === null);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        if (e instanceof Unauthorized) {
          setNeedsAuth(true);
        } else if (!(e instanceof Forbidden)) {
          // The probe endpoint is public; a 403 here is unexpected, but
          // surfaces as a banner via forbiddenAt instead of crashing
          // the gate. Anything else is a genuine probe failure.
          setError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!cancelled) setProbing(false);
      });
    return () => {
      cancelled = true;
    };
  }, [authHeader, unauthorizedAt, setRole]);

  // Periodic SSO refresh. Falls back silently to Basic if Basic is
  // available; otherwise full redirect.
  useEffect(() => {
    const proxyUrl = authConfig?.sso_proxy_url ?? null;
    if (!proxyUrl) return;
    return startTokenRefresh(proxyUrl, () => basic !== null);
  }, [authConfig?.sso_proxy_url, basic]);

  if (probing) {
    return <div className="p-4 text-text-muted">connecting…</div>;
  }
  if (error) {
    return <div className="p-4 text-danger">{error}</div>;
  }
  if (needsAuth) {
    return (
      <AuthModal
        ssoProxyUrl={authConfig?.sso_proxy_url ?? null}
        currentBasic={basic}
        initialRemember={remembered}
        onSubmit={(next, remember) => {
          setBasic(next, remember);
          clearTransients();
        }}
      />
    );
  }
  return (
    <>
      {forbiddenAt !== null && (
        <ForbiddenBanner onDismiss={clearTransients} />
      )}
      {children}
    </>
  );
}

function ForbiddenBanner({ onDismiss }: { onDismiss: () => void }) {
  return (
    <div
      role="alert"
      className="mx-6 mt-4 flex items-center justify-between rounded border border-danger/40 bg-danger/10 px-4 py-2 text-sm text-danger"
    >
      <span>
        Action requires <strong>admin</strong> role. Sign in with admin
        credentials to continue.
      </span>
      <button
        type="button"
        onClick={onDismiss}
        className="text-xs uppercase tracking-wider text-danger/80 hover:text-danger"
      >
        dismiss
      </button>
    </div>
  );
}

function AuthModal({
  ssoProxyUrl,
  currentBasic,
  initialRemember,
  onSubmit,
}: {
  ssoProxyUrl: string | null;
  currentBasic: { username: string; password: string } | null;
  initialRemember: boolean;
  onSubmit: (
    next: { username: string; password: string } | null,
    remember?: boolean,
  ) => void;
}) {
  const [username, setUsername] = useState(currentBasic?.username ?? "");
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(initialRemember);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit({ username, password }, remember);
  };
  return (
    <div className="fixed inset-0 flex items-center justify-center bg-bg/80 backdrop-blur-sm">
      <div className="w-80 rounded border border-border bg-surface p-6 shadow-xl">
        <h2 className="mb-4 text-md font-semibold">Sign in</h2>
        {ssoProxyUrl && (
          <div className="mb-4">
            <button
              type="button"
              onClick={() => redirectToSso(ssoProxyUrl)}
              className="w-full rounded bg-accent px-3 py-2 text-sm font-medium text-accent-fg hover:bg-accent-hover"
            >
              Sign in via SSO
            </button>
            <p className="mt-2 text-xs text-text-muted">
              SSO grants read-only access including logs and SQL text.
            </p>
            <div className="my-4 flex items-center gap-2 text-xs text-text-dim">
              <span className="h-px flex-1 bg-border" />
              or
              <span className="h-px flex-1 bg-border" />
            </div>
          </div>
        )}
        <form onSubmit={submit}>
          <p className="mb-4 text-sm text-text-muted">
            {currentBasic
              ? "That user/password did not work. Check [general].admin_username and [general].admin_password in pg_doorman.toml."
              : "Sign in with the admin_username / admin_password from [general] in pg_doorman.toml."}
          </p>
          <label className="mb-2 block text-xs uppercase tracking-wide text-text-muted">
            Username
          </label>
          <input
            autoFocus
            autoComplete="username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            className="mb-3 w-full rounded border border-border-strong bg-surface-2 px-2 py-1.5 text-sm text-text"
          />
          <label className="mb-2 block text-xs uppercase tracking-wide text-text-muted">
            Password
          </label>
          <input
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            className="mb-3 w-full rounded border border-border-strong bg-surface-2 px-2 py-1.5 text-sm text-text"
          />
          <label className="mb-4 flex items-center gap-2 text-sm text-text-muted">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => setRemember(e.target.checked)}
            />
            Remember me on this device
          </label>
          <button
            type="submit"
            className="w-full rounded bg-surface-2 px-3 py-1.5 text-sm font-medium text-text hover:bg-surface-3"
          >
            Sign in with Basic
          </button>
        </form>
      </div>
    </div>
  );
}
