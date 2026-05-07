/**
 * Typed fetch wrapper. Reads credentials from the AdminAuth context lazily
 * via the headerProvider param, so a credential update in AuthGate
 * propagates to in-flight retries without component remounting.
 *
 * The module also owns two single callbacks that AdminAuth registers at
 * mount:
 *
 *  - `onUnauthorized` fires on any 401 response so AuthGate can pop the
 *    sign-in modal again.
 *  - `onForbidden` fires on any 403. Credentials are valid but the role
 *    is too low; AuthGate must NOT re-prompt for login. Instead the UI
 *    shows a "needs admin role" banner.
 */
import type { AuthConfig } from "./types";

export class Unauthorized extends Error {
  constructor() {
    super("401 Unauthorized");
    this.name = "Unauthorized";
  }
}

export class Forbidden extends Error {
  constructor() {
    super("403 Forbidden");
    this.name = "Forbidden";
  }
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly body: string,
  ) {
    super(`api ${status}: ${body.slice(0, 200)}`);
    this.name = "ApiError";
  }
}

export type HeaderProvider = () => Record<string, string>;

let onUnauthorized: () => void = () => {};
let onForbidden: () => void = () => {};

export function setOnUnauthorized(cb: () => void) {
  onUnauthorized = cb;
}

export function setOnForbidden(cb: () => void) {
  onForbidden = cb;
}

function buildHeaders(provided: Record<string, string>): Record<string, string> {
  // When we have no credentials we still set an explicit (empty)
  // Authorization header to override the browser's basic-auth cache. The
  // override is skipped when the caller provided an Authorization header
  // (Bearer for SSO, Basic for admin) so we don't clobber a real
  // credential.
  const hasAuth = Object.keys(provided).some(
    (k) => k.toLowerCase() === "authorization",
  );
  return {
    Accept: "application/json",
    ...(hasAuth ? {} : { Authorization: "Basic " }),
    ...provided,
  };
}

export async function apiGet<T>(
  path: string,
  headerProvider: HeaderProvider,
  signal?: AbortSignal,
): Promise<T> {
  const headers = buildHeaders(headerProvider());
  const res = await fetch(path, {
    method: "GET",
    credentials: "omit",
    headers,
    signal,
  });
  if (res.status === 401) {
    onUnauthorized();
    throw new Unauthorized();
  }
  if (res.status === 403) {
    onForbidden();
    throw new Forbidden();
  }
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return (await res.json()) as T;
}

export async function apiPost<T>(
  path: string,
  headerProvider: HeaderProvider,
): Promise<T> {
  const headers = buildHeaders(headerProvider());
  const res = await fetch(path, {
    method: "POST",
    credentials: "omit",
    headers,
  });
  if (res.status === 401) {
    onUnauthorized();
    throw new Unauthorized();
  }
  if (res.status === 403) {
    onForbidden();
    throw new Forbidden();
  }
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return (await res.json()) as T;
}

/**
 * Public probe used by AuthGate on mount and after a Basic credentials
 * change. Returns the SSO config and, when the request was authenticated,
 * the current identity + role.
 */
export async function fetchAuthConfig(
  headerProvider: HeaderProvider,
  signal?: AbortSignal,
): Promise<AuthConfig> {
  return apiGet<AuthConfig>("/api/auth/config", headerProvider, signal);
}
