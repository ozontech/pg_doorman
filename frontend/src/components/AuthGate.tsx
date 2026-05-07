import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
} from "react";
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
 * On 401 the modal re-opens. On 403 it does not: credentials are
 * valid, the role is just too low. The UI raises a forbidden banner
 * instead so the operator can see why the action was blocked without
 * losing their session.
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
  const [hadFirstResponse, setHadFirstResponse] = useState(false);

  // Capture `?token=` returned by the SSO proxy on first load. The
  // captureTokenFromUrl helper rewrites the URL clean of the param so
  // the rest of the SPA does not see it.
  useEffect(() => {
    if (captureTokenFromUrl()) {
      setSsoToken(getValidSsoToken());
    }
  }, [setSsoToken]);

  // Probe /api/auth/config on mount, on Basic credentials change, and
  // any time api.ts saw a 401 elsewhere. Uses an AbortController so a
  // rapid sequence of credential changes does not pile probes on top
  // of each other; the latest one wins.
  useEffect(() => {
    const ctrl = new AbortController();
    setProbing(true);
    setError(null);
    fetchAuthConfig(authHeader, ctrl.signal)
      .then((cfg) => {
        if (ctrl.signal.aborted) return;
        setAuthConfig(cfg);
        const role = cfg.current_user?.role ?? "anonymous";
        setRole(role);
        setNeedsAuth(cfg.current_user === null);
        setHadFirstResponse(true);
      })
      .catch((e: unknown) => {
        if (ctrl.signal.aborted) return;
        if (e instanceof Error && e.name === "AbortError") return;
        if (e instanceof Unauthorized) {
          setNeedsAuth(true);
          setHadFirstResponse(true);
        } else if (!(e instanceof Forbidden)) {
          // Probe is public; a 403 here is unexpected. Let `forbiddenAt`
          // raise the banner. Anything else is a genuine probe failure.
          setError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!ctrl.signal.aborted) setProbing(false);
      });
    return () => ctrl.abort();
  }, [authHeader, unauthorizedAt, setRole]);

  // Periodic SSO refresh. Falls back silently to Basic if Basic is
  // available; otherwise full redirect.
  useEffect(() => {
    const proxyUrl = authConfig?.sso_proxy_url ?? null;
    if (!proxyUrl) return;
    return startTokenRefresh(proxyUrl, () => basic !== null);
  }, [authConfig?.sso_proxy_url, basic]);

  // First load shows a placeholder. Subsequent re-probes (after a
  // credential change or 401) keep the previous content visible behind
  // a translucent overlay so an operator's scroll position and form
  // state do not get lost mid-session.
  if (!hadFirstResponse && probing) {
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
    <div className="relative">
      {forbiddenAt !== null && (
        <ForbiddenBanner onDismiss={clearTransients} />
      )}
      {children}
      {probing && (
        <div
          className="pointer-events-none absolute inset-0 bg-bg/30"
          aria-hidden="true"
        />
      )}
    </div>
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
        aria-label="Dismiss admin role notice"
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
  const [redirecting, setRedirecting] = useState(false);
  const dialogRef = useRef<HTMLDivElement | null>(null);

  // Minimal focus trap: keep Tab inside the dialog. Without this, Tab
  // reaches the (occluded) main content and a keyboard-only operator
  // can lose track of focus. Picks up every focusable element in the
  // dialog on each Tab so dynamic UI (the redirecting button) stays
  // reachable.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Tab") return;
    const root = dialogRef.current;
    if (!root) return;
    const focusable = root.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])',
    );
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement as HTMLElement | null;
    if (e.shiftKey && active === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSubmit({ username, password }, remember);
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="auth-modal-title"
      ref={dialogRef}
      onKeyDown={onKeyDown}
      className="fixed inset-0 flex items-center justify-center bg-bg/80 backdrop-blur-sm"
    >
      <div className="w-80 rounded border border-border bg-surface p-6 shadow-xl">
        <h2 id="auth-modal-title" className="mb-4 text-md font-semibold">
          Sign in
        </h2>
        {ssoProxyUrl && (
          <div className="mb-4">
            <button
              type="button"
              disabled={redirecting}
              onClick={() => {
                setRedirecting(true);
                redirectToSso(ssoProxyUrl);
              }}
              className="w-full rounded bg-accent px-3 py-2 text-sm font-medium text-accent-fg hover:bg-accent-hover disabled:opacity-60"
            >
              {redirecting ? "Redirecting…" : "Sign in via SSO"}
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
