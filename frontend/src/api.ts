/**
 * Typed fetch wrapper. Reads credentials from the AdminAuth context lazily
 * via the headerProvider param, so a credential update in AuthGate
 * propagates to in-flight retries without component remounting.
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
  if (res.status === 401) throw new Unauthorized();
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return (await res.json()) as T;
}
