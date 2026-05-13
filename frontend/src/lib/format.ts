// Shared formatters extracted from Sidebar / Wall / Overview / Servers
// duplicates. Operators reading the SPA see the same string shape for
// the same quantity everywhere; previously each page had its own
// near-identical helpers, so 1.2 MiB on one screen sometimes read as
// "1.20 MiB" or "1.2MiB" on another.

/** Compact rate formatter — number + k / M suffix, no whitespace. */
export function fmtRate(n: number | undefined, unit?: string): string {
  if (n === undefined) return "—";
  const abs = Math.abs(n);
  let body: string;
  if (abs >= 1_000_000) body = `${(n / 1_000_000).toFixed(1)}M`;
  else if (abs >= 10_000) body = `${(n / 1000).toFixed(0)}k`;
  else if (abs >= 1000) body = `${(n / 1000).toFixed(1)}k`;
  else if (abs >= 10) body = n.toFixed(0);
  else body = n.toFixed(2);
  return unit ? `${body}${unit}` : body;
}

/** Compact duration formatter — ms / s / m / h with no whitespace. */
export function fmtMs(n: number | undefined): string {
  if (n === undefined) return "—";
  if (n > 0 && n < 1) return `${n.toFixed(2)}ms`;
  if (n < 10) return `${n.toFixed(1)}ms`;
  if (n < 1000) return `${Math.round(n)}ms`;
  if (n < 10_000) return `${(n / 1000).toFixed(1)}s`;
  if (n < 60_000) return `${Math.round(n / 1000)}s`;
  if (n < 3_600_000) {
    const m = Math.floor(n / 60_000);
    const s = Math.floor((n % 60_000) / 1000);
    return `${m}m${s.toString().padStart(2, "0")}s`;
  }
  const h = Math.floor(n / 3_600_000);
  const m = Math.floor((n % 3_600_000) / 60_000);
  return `${h}h${m.toString().padStart(2, "0")}m`;
}

/** Bytes with binary suffix. Falls back to "B" for sub-KiB values. */
export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MiB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GiB`;
}

/** Human-friendly process uptime. */
export function fmtUptime(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  const d = Math.floor(h / 24);
  return `${d}d ${h % 24}h`;
}

/** Relative "Ns ago" / "Nm ago" / "now" formatter for last-update chips. */
export function fmtAge(ts: number | null): string {
  if (!ts) return "—";
  const ageSec = Math.round((Date.now() - ts) / 1000);
  if (ageSec < 5) return "now";
  if (ageSec < 60) return `${ageSec}s ago`;
  return `${Math.round(ageSec / 60)}m ago`;
}

/** Clock formatter HH:MM:SS — used by event tickers. */
export function fmtClock(tsMs: number): string {
  const d = new Date(tsMs);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}
