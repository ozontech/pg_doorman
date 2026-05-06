/**
 * Typed fetch wrapper. Reads credentials from the AdminAuth context lazily
 * via the headerProvider param, so a credential update in AuthGate
 * propagates to in-flight retries without component remounting.
 *
 * The module also owns a single `onUnauthorized` callback that AdminAuth
 * registers at mount. Any 401 response — from anywhere in the app, not just
 * the AuthGate's version probe — fires the callback so the gate can pop the
 * sign-in modal again. Without this hook, an admin-only page that 401's
 * after AuthGate already greenlit would leave the operator stuck on a red
 * error message.
 */
export class Unauthorized extends Error {
  constructor() {
    super("401 Unauthorized");
    this.name = "Unauthorized";
  }
}

export class ApiError extends Error {
  constructor(public readonly status: number, public readonly body: string) {
    super(`api ${status}: ${body.slice(0, 200)}`);
    this.name = "ApiError";
  }
}

export type HeaderProvider = () => Record<string, string>;

let onUnauthorized: () => void = () => {};

export function setOnUnauthorized(cb: () => void) {
  onUnauthorized = cb;
}

export async function apiGet<T>(
  path: string,
  headerProvider: HeaderProvider,
  signal?: AbortSignal,
): Promise<T> {
  const res = await fetch(path, {
    method: "GET",
    headers: { Accept: "application/json", ...headerProvider() },
    signal,
  });
  if (res.status === 401) {
    onUnauthorized();
    throw new Unauthorized();
  }
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return (await res.json()) as T;
}
