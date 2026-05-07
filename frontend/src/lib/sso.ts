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

/** Capture a `?token=...` returned by the SSO proxy and clean the URL. */
export function captureTokenFromUrl(): boolean {
  const params = new URLSearchParams(window.location.search);
  const token = params.get("token");
  if (!token) return false;
  localStorage.setItem(SSO_TOKEN_KEY, token);
  params.delete("token");
  const qs = params.toString();
  const newUrl = qs
    ? `${window.location.pathname}?${qs}`
    : window.location.pathname;
  window.history.replaceState({}, "", newUrl);
  return true;
}

/** Send the user agent to the SSO proxy with the current href as redirect target. */
export function redirectToSso(proxyUrl: string): void {
  const url = new URL(proxyUrl);
  url.searchParams.set("redirect_to", window.location.href);
  window.location.href = url.toString();
}

let refreshInFlight: Promise<boolean> | null = null;

/**
 * Ask the SSO proxy for a fresh token through a hidden iframe. Resolves
 * `true` once the iframe posts the new token back via `window.postMessage`,
 * `false` if no message arrives within `SILENT_REFRESH_TIMEOUT_MS`.
 *
 * The iframe lands on `${origin}/?sso_silent=1`; App.tsx detects that
 * sentinel and renders <SilentCallback /> which captures the token and
 * calls `window.parent.postMessage({type:"sso-token", token})`.
 *
 * Concurrent callers share a single Promise so two timer ticks cannot
 * spawn duplicate iframes.
 */
export function silentRefresh(proxyUrl: string): Promise<boolean> {
  if (refreshInFlight) return refreshInFlight;

  refreshInFlight = new Promise<boolean>((resolve) => {
    const iframe = document.createElement("iframe");
    iframe.style.display = "none";
    let settled = false;

    const cleanup = () => {
      settled = true;
      refreshInFlight = null;
      window.removeEventListener("message", onMessage);
      clearTimeout(timer);
      // Defer DOM removal so the iframe gets a chance to finish loading;
      // tearing it down mid-load occasionally races with the postMessage
      // delivery on Firefox.
      setTimeout(() => {
        try {
          document.body.removeChild(iframe);
        } catch {
          // already removed
        }
      }, 100);
    };

    const onMessage = (ev: MessageEvent) => {
      if (ev.origin !== window.location.origin) return;
      if (settled) return;
      const data = ev.data as { type?: string; token?: string } | null;
      if (data?.type !== "sso-token") return;
      const token = data.token;
      if (typeof token === "string" && token.length > 0) {
        localStorage.setItem(SSO_TOKEN_KEY, token);
        cleanup();
        resolve(true);
      }
    };

    const timer = setTimeout(() => {
      if (settled) return;
      cleanup();
      resolve(false);
    }, SILENT_REFRESH_TIMEOUT_MS);

    window.addEventListener("message", onMessage);

    const callbackUrl = new URL(window.location.origin);
    callbackUrl.searchParams.set("sso_silent", "1");
    const ssoUrl = new URL(proxyUrl);
    ssoUrl.searchParams.set("redirect_to", callbackUrl.toString());
    iframe.src = ssoUrl.toString();
    document.body.appendChild(iframe);
  });

  return refreshInFlight;
}

/**
 * Periodic check: when the SSO token is < REFRESH_MARGIN_SEC from
 * expiring, attempt silent refresh. On failure, fall back to a full
 * redirect, unless `onFallbackBlocked` returns true (typically because
 * the operator still has working Basic credentials and we'd rather drop
 * the dead SSO token than punt them through the proxy).
 *
 * Returns a cleanup function that cancels the interval.
 */
export function startTokenRefresh(
  proxyUrl: string,
  onFallbackBlocked?: () => boolean,
): () => void {
  const id = window.setInterval(async () => {
    const token = getValidSsoToken();
    if (!token) return;
    const parsed = parseJwt(token);
    if (!parsed || typeof parsed.exp !== "number") return;
    const remaining = parsed.exp - Math.floor(Date.now() / 1000);
    if (remaining >= REFRESH_MARGIN_SEC) return;
    const ok = await silentRefresh(proxyUrl);
    if (ok) return;
    if (onFallbackBlocked && onFallbackBlocked()) {
      localStorage.removeItem(SSO_TOKEN_KEY);
      return;
    }
    redirectToSso(proxyUrl);
  }, POLL_INTERVAL_MS);
  return () => window.clearInterval(id);
}
