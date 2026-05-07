// SSO redirect helpers ported from rpglot. The flow:
//
//   1. The SPA reads /api/auth/config on mount.
//   2. If sso_proxy_url is set and there is no valid token, the user
//      hits the "Sign in via SSO" button → `redirectToSso` sends them
//      to the proxy with `redirect_to=<current href>`.
//   3. The proxy authenticates the user and redirects back with
//      `?token=<jwt>` in the URL. `captureTokenFromUrl` stores the
//      token in localStorage and rewrites the URL clean of the token.
//   4. `startTokenRefresh` polls the token's `exp` and triggers a
//      hidden-iframe silent refresh ~90s before expiry. The iframe
//      lands on `?sso_silent=1`, which the App routes to a minimal
//      <SilentCallback> component (no normal UI). That component
//      captures the new token and posts it back via window.postMessage.

import { parseJwt, SSO_TOKEN_KEY, getValidSsoToken } from "./jwt";

const REFRESH_MARGIN_SEC = 90;
const SILENT_REFRESH_TIMEOUT_MS = 10_000;
const POLL_INTERVAL_MS = 60_000;

/**
 * Capture a `?token=...` returned by the SSO proxy and clean the URL.
 * Returns the captured token on success so the caller can feed it
 * into React state in the same render. Falls back to in-memory only
 * when localStorage is unavailable (private mode / quota); the caller
 * still receives the token and the session works until reload.
 */
export function captureTokenFromUrl(): string | null {
  const params = new URLSearchParams(window.location.search);
  const token = params.get("token");
  if (!token) return null;
  // Always rewrite the URL clean of `?token=`, even when the value is
  // garbage — otherwise a bad redirect loops on the same broken token
  // every time the SPA re-mounts.
  params.delete("token");
  const qs = params.toString();
  const newUrl = qs
    ? `${window.location.pathname}?${qs}`
    : window.location.pathname;
  window.history.replaceState({}, "", newUrl);
  if (!parseJwt(token)) {
    // Shape-valid JWT only. Backend will reject signature anyway, but
    // we refuse to feed obvious junk into localStorage / Authorization.
    return null;
  }
  try {
    localStorage.setItem(SSO_TOKEN_KEY, token);
  } catch {
    /* private mode / quota — non-fatal. The caller still gets the
     * token returned from this function and can drive React state;
     * the session will not survive a reload, but the SPA works for
     * the current load. */
  }
  return token;
}

/**
 * Send the user agent to the SSO proxy with the current href as
 * redirect target. Validates the proxy URL: must parse, must use https
 * (or be localhost for development). A bad URL logs to the console
 * and aborts the redirect, so a typo in `pg_doorman.toml` shows in
 * devtools instead of leaving the SPA stuck on a half-redirect.
 *
 * Returns `true` when navigation was scheduled, `false` when the URL
 * was rejected — the caller can use this to clear a "Redirecting…"
 * spinner instead of leaving the button stuck.
 */
export function redirectToSso(proxyUrl: string): boolean {
  const url = safeProxyUrl(proxyUrl);
  if (!url) return false;
  url.searchParams.set("redirect_to", window.location.href);
  window.location.href = url.toString();
  return true;
}

function safeProxyUrl(proxyUrl: string): URL | null {
  let url: URL;
  try {
    url = new URL(proxyUrl);
  } catch {
    console.error("sso_proxy_url is not a valid URL:", proxyUrl);
    return null;
  }
  const isLocal = url.hostname === "localhost" || url.hostname === "127.0.0.1";
  if (url.protocol !== "https:" && !isLocal) {
    console.error(
      "sso_proxy_url must use https (got",
      url.protocol,
      "for",
      url.hostname,
      ")",
    );
    return null;
  }
  return url;
}

interface SsoTokenMessage {
  type: "sso-token";
  token: string;
}

function isSsoTokenMessage(d: unknown): d is SsoTokenMessage {
  if (typeof d !== "object" || d === null) return false;
  const obj = d as Record<string, unknown>;
  return obj.type === "sso-token" && typeof obj.token === "string";
}

let refreshInFlight: Promise<string | null> | null = null;

/**
 * Ask the SSO proxy for a fresh token through a hidden iframe. Resolves
 * with the new JWT once the iframe posts it back via
 * `window.postMessage`, or `null` if no message arrives within
 * `SILENT_REFRESH_TIMEOUT_MS` or if `signal` aborts before then.
 *
 * The iframe lands on `${origin}/?sso_silent=1`; App.tsx detects that
 * sentinel and renders <SilentCallback /> which captures the token and
 * calls `window.parent.postMessage({type:"sso-token", token})`.
 *
 * Concurrent callers share a single Promise so two timer ticks cannot
 * spawn duplicate iframes. Returning the token (rather than just `true`)
 * lets the caller propagate it into React state — `storage` events do
 * not fire in the originating tab, so a write to localStorage alone
 * leaves `useAdminAuth` holding the stale token.
 */
export function silentRefresh(
  proxyUrl: string,
  signal?: AbortSignal,
): Promise<string | null> {
  if (refreshInFlight) return refreshInFlight;

  refreshInFlight = new Promise<string | null>((resolve) => {
    const iframe = document.createElement("iframe");
    iframe.style.display = "none";
    let settled = false;

    const cleanup = () => {
      settled = true;
      window.removeEventListener("message", onMessage);
      if (signal) signal.removeEventListener("abort", onAbort);
      clearTimeout(timer);
      // Firefox drops the postMessage if the iframe is removed in the
      // same task as the dispatch. 100ms gives the message time to
      // land. We also clear `refreshInFlight` after the removal so a
      // follow-up caller cannot spawn a second iframe while the first
      // still sits in the DOM.
      setTimeout(() => {
        try {
          document.body.removeChild(iframe);
        } catch {
          // already removed
        }
        refreshInFlight = null;
      }, 100);
    };

    const onMessage = (ev: MessageEvent) => {
      if (ev.origin !== window.location.origin) return;
      if (settled) return;
      if (!isSsoTokenMessage(ev.data)) return;
      try {
        localStorage.setItem(SSO_TOKEN_KEY, ev.data.token);
      } catch {
        /* non-fatal */
      }
      const token = ev.data.token;
      cleanup();
      resolve(token);
    };

    const onAbort = () => {
      if (settled) return;
      cleanup();
      resolve(null);
    };

    const timer = setTimeout(() => {
      if (settled) return;
      cleanup();
      resolve(null);
    }, SILENT_REFRESH_TIMEOUT_MS);

    window.addEventListener("message", onMessage);
    if (signal) {
      if (signal.aborted) {
        onAbort();
        return;
      }
      signal.addEventListener("abort", onAbort);
    }

    const ssoUrl = safeProxyUrl(proxyUrl);
    if (!ssoUrl) {
      cleanup();
      resolve(null);
      return;
    }
    const callbackUrl = new URL(window.location.origin);
    callbackUrl.searchParams.set("sso_silent", "1");
    ssoUrl.searchParams.set("redirect_to", callbackUrl.toString());
    iframe.src = ssoUrl.toString();
    document.body.appendChild(iframe);
  });

  return refreshInFlight;
}

/**
 * Periodic check: when the SSO token is < REFRESH_MARGIN_SEC from
 * expiring, attempt silent refresh. On success the callback `onToken`
 * lets the caller propagate the new token into React state — writing
 * to localStorage alone is not enough, because `storage` events do
 * not fire in the originating tab. On failure, fall back to a full
 * redirect, unless `onFallbackBlocked` returns true (typically because
 * the operator still has working Basic credentials and we'd rather
 * drop the dead SSO token than push them through the proxy).
 *
 * The interval pauses while `document.hidden` is true: hidden tabs
 * already throttle setInterval to ~1Hz and a refresh request would
 * just waste an iframe and a network round-trip until the operator
 * comes back. On `visibilitychange` we check immediately so a long
 * idle does not leave the operator with an expired token.
 *
 * Returns a cleanup function that cancels the interval and aborts any
 * silent refresh that is currently in flight.
 */
export function startTokenRefresh(
  proxyUrl: string,
  onToken: (token: string) => void,
  onFallbackBlocked?: () => boolean,
): () => void {
  const ctrl = new AbortController();
  let intervalId: number | null = null;
  let runningCheck = false;

  const tick = async () => {
    if (ctrl.signal.aborted) return;
    if (runningCheck) return;
    runningCheck = true;
    try {
      const token = getValidSsoToken();
      if (!token) return;
      const parsed = parseJwt(token);
      if (!parsed || typeof parsed.exp !== "number") return;
      const remaining = parsed.exp - Math.floor(Date.now() / 1000);
      if (remaining >= REFRESH_MARGIN_SEC) return;
      const fresh = await silentRefresh(proxyUrl, ctrl.signal);
      if (ctrl.signal.aborted) return;
      if (fresh) {
        onToken(fresh);
        return;
      }
      if (onFallbackBlocked && onFallbackBlocked()) {
        try {
          localStorage.removeItem(SSO_TOKEN_KEY);
        } catch {
          /* non-fatal */
        }
        return;
      }
      redirectToSso(proxyUrl);
    } finally {
      runningCheck = false;
    }
  };

  const startInterval = () => {
    if (intervalId !== null) return;
    intervalId = window.setInterval(tick, POLL_INTERVAL_MS);
  };
  const stopInterval = () => {
    if (intervalId === null) return;
    window.clearInterval(intervalId);
    intervalId = null;
  };

  const onVisibility = () => {
    if (document.hidden) {
      stopInterval();
    } else {
      // The tab just came back into focus — re-check immediately, then
      // resume the regular cadence.
      void tick();
      startInterval();
    }
  };

  if (!document.hidden) startInterval();
  document.addEventListener("visibilitychange", onVisibility);

  return () => {
    ctrl.abort();
    stopInterval();
    document.removeEventListener("visibilitychange", onVisibility);
  };
}
