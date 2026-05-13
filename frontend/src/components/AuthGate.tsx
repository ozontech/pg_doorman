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
  // helper returns the token directly so we can drive React state
  // even when localStorage is unavailable (private mode / quota).
  //
  // A successful SSO callback also clears any persisted Basic
  // credentials. authHeader prefers Basic over SSO, so leaving stale
  // creds (e.g. after an admin password rotation, or the operator
  // logged out elsewhere and ticked "remember me" earlier) would mask
  // the fresh JWT and every API call would 401. Explicitly choosing
  // SSO via the proxy is a strong signal to drop Basic.
  useEffect(() => {
    const captured = captureTokenFromUrl();
    if (captured) {
      setSsoToken(captured);
      setBasic(null, false);
    }
  }, [setSsoToken, setBasic]);

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
        // /api/auth/config is public, so a null current_user only tells
        // us the request was anonymous. The backend still gates per
        // path: anonymous can read public API endpoints when the listener
        // was started with `[web].ui_anonymous = true`. Re-arm the
        // sign-in modal only when api.ts has seen a real 401 elsewhere
        // (`unauthorizedAt` bumped) and the operator is still anonymous
        // — otherwise the SPA stays open in Anonymous mode.
        setNeedsAuth(cfg.current_user === null && unauthorizedAt !== null);
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

  // Periodic SSO refresh. The onToken callback drives React state in
  // the same tab — `storage` events do not fire here. Falls back to
  // Basic when available; otherwise full redirect.
  useEffect(() => {
    const proxyUrl = authConfig?.sso_proxy_url ?? null;
    if (!proxyUrl) return;
    return startTokenRefresh(
      proxyUrl,
      (token) => setSsoToken(token),
      () => basic !== null,
    );
  }, [authConfig?.sso_proxy_url, basic, setSsoToken]);

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
  const ssoConfigError = authConfig?.sso_config_error ?? null;
  if (needsAuth) {
    return (
      <AuthModal
        ssoProxyUrl={authConfig?.sso_proxy_url ?? null}
        ssoConfigError={ssoConfigError}
        ssoAdminPossible={hasAdminGroupsConfig(authConfig)}
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
      {ssoConfigError && <SsoConfigErrorBanner reason={ssoConfigError} />}
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

/// Whether the backend has `[web].sso_admin_groups` configured. The
/// SPA uses this to soften the sign-in modal copy when SSO can
/// resolve to Admin via group membership. The actual role is still
/// decided server-side when the JWT lands.
function hasAdminGroupsConfig(cfg: AuthConfig | null): boolean {
  return cfg?.sso_admin_groups_configured === true;
}

function SsoConfigErrorBanner({ reason }: { reason: string }) {
  return (
    <div
      role="alert"
      className="mx-6 mt-4 flex items-center justify-between rounded border border-warning/40 bg-warning/10 px-4 py-2 text-sm text-warning"
    >
      <span>
        SSO is configured but not loaded:{" "}
        <strong className="font-mono">{reason}</strong>. Backend serves
        Basic auth only until this is fixed.
      </span>
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
  ssoConfigError,
  ssoAdminPossible,
  currentBasic,
  initialRemember,
  onSubmit,
}: {
  ssoProxyUrl: string | null;
  ssoConfigError: string | null;
  ssoAdminPossible: boolean;
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

  // Sign-in view. Uses the console palette without terminal-style decoration.
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="auth-modal-title"
      ref={dialogRef}
      onKeyDown={onKeyDown}
      className="fixed inset-0 flex min-h-screen items-center justify-center bg-bg/95 px-4 py-8 backdrop-blur-sm"
    >
      <section className="w-full max-w-md border border-border bg-surface">
        <header className="border-b border-border px-8 pt-7 pb-5">
          <p className="text-xs text-text-dim">pg_doorman</p>
          <h2
            id="auth-modal-title"
            className="mt-1 text-2xl font-semibold tracking-tight text-text"
          >
            Sign in
          </h2>
          <p className="mt-2 text-sm text-text-muted">
            Authenticate to open the operator console.
          </p>
        </header>
        <div className="space-y-6 px-8 py-6">
          {ssoConfigError && (
            <div
              role="alert"
              className="border border-warning/40 bg-warning/10 px-3 py-2 text-xs leading-relaxed text-warning"
            >
              <span className="font-semibold uppercase tracking-wide">
                SSO not loaded
              </span>{" "}
              · <span className="font-mono">{ssoConfigError}</span>. Basic
              auth below still works until this is fixed.
            </div>
          )}
          {ssoProxyUrl && (
            <SsoBlock
              proxyUrl={ssoProxyUrl}
              ssoAdminPossible={ssoAdminPossible}
              redirecting={redirecting}
              onRedirect={() => {
                setRedirecting(true);
                if (!redirectToSso(ssoProxyUrl)) {
                  // Bad sso_proxy_url; the helper logged to console.
                  // Reset the spinner so the operator can retry Basic.
                  setRedirecting(false);
                }
              }}
            />
          )}
          <BasicBlock
            currentBasic={currentBasic}
            username={username}
            password={password}
            remember={remember}
            onUsername={setUsername}
            onPassword={setPassword}
            onRemember={setRemember}
            onSubmit={submit}
            ssoVisible={ssoProxyUrl !== null}
          />
        </div>
        <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-border px-8 py-3 text-xs text-text-dim">
          <TransportChip />
          <span>{ssoProxyUrl ? "SSO + Basic" : "Basic only"}</span>
        </footer>
      </section>
    </div>
  );
}

function SsoBlock({
  proxyUrl,
  ssoAdminPossible,
  redirecting,
  onRedirect,
}: {
  proxyUrl: string;
  ssoAdminPossible: boolean;
  redirecting: boolean;
  onRedirect: () => void;
}) {
  // Trim the proxy host out of the URL so the operator sees where SSO
  // will route them before clicking. The try/catch handles a typo'd
  // sso_proxy_url at render time — the runtime `safeProxyUrl` check
  // only fires when the operator actually clicks the button.
  let host: string | null = null;
  try {
    host = new URL(proxyUrl).host;
  } catch {
    host = null;
  }
  return (
    <div>
      <button
        type="button"
        disabled={redirecting}
        onClick={onRedirect}
        className="flex h-11 w-full items-center justify-center gap-2 border border-accent bg-accent px-4 text-sm font-semibold text-accent-fg transition-colors hover:bg-accent-hover focus-visible:bg-accent-hover disabled:cursor-wait disabled:opacity-70"
      >
        <span>{redirecting ? "Redirecting…" : "Continue with SSO"}</span>
        {!redirecting && (
          <span aria-hidden="true" className="font-mono">
            →
          </span>
        )}
      </button>
      <p className="mt-2 text-xs leading-relaxed text-text-muted">
        {host ? (
          <>
            Routes via <span className="font-mono text-text">{host}</span>.{" "}
          </>
        ) : null}
        {ssoAdminPossible
          ? "Group membership in the JWT decides whether you get read-only SSO or full admin."
          : "SSO grants read-only access including logs and SQL text."}
      </p>
    </div>
  );
}

function BasicBlock({
  currentBasic,
  username,
  password,
  remember,
  onUsername,
  onPassword,
  onRemember,
  onSubmit,
  ssoVisible,
}: {
  currentBasic: { username: string; password: string } | null;
  username: string;
  password: string;
  remember: boolean;
  onUsername: (next: string) => void;
  onPassword: (next: string) => void;
  onRemember: (next: boolean) => void;
  onSubmit: (e: FormEvent) => void;
  ssoVisible: boolean;
}) {
  return (
    <div>
      {ssoVisible && (
        <div
          className="mb-3 flex items-center gap-3 text-xs text-text-dim"
          aria-hidden="true"
        >
          <span className="h-px flex-1 bg-border" />
          <span>or local admin</span>
          <span className="h-px flex-1 bg-border" />
        </div>
      )}
      {!ssoVisible && (
        <p className="mb-3 text-xs text-text-dim">Local admin</p>
      )}
      {currentBasic && (
        <p
          role="alert"
          className="mb-3 border border-danger/30 bg-danger/10 px-3 py-2 text-xs leading-relaxed text-danger"
        >
          That user/password was rejected. Recheck{" "}
          <span className="font-mono">[general].admin_username</span> and{" "}
          <span className="font-mono">[general].admin_password</span> in{" "}
          <span className="font-mono">pg_doorman.toml</span>.
        </p>
      )}
      <form onSubmit={onSubmit} className="space-y-3">
        <div>
          <label
            htmlFor="auth-username"
            className="mb-1 block text-xs text-text-muted"
          >
            Username
          </label>
          <input
            id="auth-username"
            autoFocus
            autoComplete="username"
            value={username}
            onChange={(e) => onUsername(e.target.value)}
            className="block h-10 w-full border border-border-strong bg-surface-2 px-3 text-sm text-text focus:border-accent focus:outline-none"
          />
        </div>
        <div>
          <label
            htmlFor="auth-password"
            className="mb-1 block text-xs text-text-muted"
          >
            Password
          </label>
          <input
            id="auth-password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => onPassword(e.target.value)}
            className="block h-10 w-full border border-border-strong bg-surface-2 px-3 text-sm text-text focus:border-accent focus:outline-none"
          />
        </div>
        <label className="flex items-center gap-2 text-sm text-text-muted">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => onRemember(e.target.checked)}
            className="h-4 w-4 accent-accent"
          />
          Remember me on this device
        </label>
        <button
          type="submit"
          className="flex h-11 w-full items-center justify-center border border-border-strong bg-surface-2 text-sm font-semibold text-text transition-colors hover:bg-surface-3 hover:border-accent focus-visible:border-accent"
        >
          Sign in
        </button>
      </form>
    </div>
  );
}

function TransportChip() {
  // Read the live protocol so the sign-in form can warn on plain HTTP.
  // SSR/test renders fall back to the warning state.
  const protocol =
    typeof window !== "undefined" ? window.location.protocol : "";
  const secure = protocol === "https:";
  const className = secure
    ? "border-success/40 text-success"
    : "border-warning/40 text-warning";
  return (
    <span
      className={`inline-flex items-center gap-2 border px-2 py-1 font-mono ${className}`}
    >
      <span className={`h-1.5 w-1.5 rounded-full ${secure ? "bg-success" : "bg-warning"}`} aria-hidden="true" />
      transport · {secure ? "https" : "http"}
    </span>
  );
}
