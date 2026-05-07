// Client-side JWT helpers. The browser never validates the signature;
// that is the backend's job. We only parse the payload to read `exp`
// and pick the username for the role-aware UI.

export const SSO_TOKEN_KEY = "pgdoorman.sso-token";

/**
 * Safe localStorage helpers. Some browsers (locked-down corporate
 * profiles, tracking-prevention modes, embedded webviews) throw
 * `SecurityError` from `getItem`/`setItem`/`removeItem`. The SSO flow
 * already tolerates a fall-through to in-memory state, so swallowing
 * the exception here is the right answer.
 */
export function safeLocalGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function safeLocalSet(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* private mode / quota / blocked — non-fatal */
  }
}

export function safeLocalRemove(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* private mode / quota / blocked — non-fatal */
  }
}

/** Parse a JWT payload without signature verification (client-side only). */
export function parseJwt(token: string): Record<string, unknown> | null {
  try {
    const parts = token.split(".");
    if (parts.length !== 3) return null;
    let b64 = parts[1].replace(/-/g, "+").replace(/_/g, "/");
    while (b64.length % 4 !== 0) b64 += "=";
    const binary = atob(b64);
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    const payload = new TextDecoder().decode(bytes);
    return JSON.parse(payload);
  } catch {
    return null;
  }
}

/**
 * Read the stored SSO token, returning null when missing or expired.
 * An expired token is also removed from localStorage so the next read
 * is consistently null.
 */
export function getValidSsoToken(): string | null {
  const token = safeLocalGet(SSO_TOKEN_KEY);
  if (!token) return null;
  const parsed = parseJwt(token);
  if (!parsed || typeof parsed.exp !== "number") {
    safeLocalRemove(SSO_TOKEN_KEY);
    return null;
  }
  if (parsed.exp <= Math.floor(Date.now() / 1000)) {
    safeLocalRemove(SSO_TOKEN_KEY);
    return null;
  }
  return token;
}

/** Resolve the username encoded in the current SSO token, or null. */
export function getSsoTokenUsername(): string | null {
  const token = getValidSsoToken();
  if (!token) return null;
  const parsed = parseJwt(token);
  if (!parsed) return null;
  if (typeof parsed.preferred_username === "string") {
    return parsed.preferred_username;
  }
  if (typeof parsed.sub === "string") return parsed.sub;
  return null;
}

export function clearSsoToken(): void {
  safeLocalRemove(SSO_TOKEN_KEY);
}
