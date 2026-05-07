import { useEffect } from "react";
import { captureTokenFromUrl } from "../lib/sso";
import { getValidSsoToken } from "../lib/jwt";

/**
 * Rendered inside the silent-refresh iframe (when the URL has
 * `?sso_silent=1`). Captures the new `?token=` from the URL, posts it
 * back to the parent window via postMessage, and otherwise renders
 * nothing visible. The parent (AuthGate's silent-refresh listener)
 * picks up the token and tears the iframe down.
 *
 * Mounting this component instead of the regular App stops every
 * normal `useEffect` (snapshot polling, schema fetch, …) from running
 * inside the iframe, where they'd be wasted requests.
 */
export function SilentCallback() {
  useEffect(() => {
    captureTokenFromUrl();
    const token = getValidSsoToken();
    if (token && window.parent !== window) {
      window.parent.postMessage(
        { type: "sso-token", token },
        window.location.origin,
      );
    }
  }, []);
  return <div data-testid="sso-silent-callback" />;
}
