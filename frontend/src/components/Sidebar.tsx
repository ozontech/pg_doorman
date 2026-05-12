import { useEffect, useMemo, useRef, useState } from "react";
import { Link, NavLink, useLocation } from "react-router-dom";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { getSsoTokenUsername } from "../lib/jwt";
import { aggregateHealth } from "../lib/thresholds";
import type {
  HottestDatabaseDto,
  OverviewDto,
  PoolsDto,
  ProcessDto,
  VersionDto,
} from "../types";

type NavItem = { to: string; label: string; personal?: boolean };

function signedInLabel(
  basic: { username: string } | null,
  ssoToken: string | null,
): string {
  if (basic) return basic.username;
  if (ssoToken) {
    const name = getSsoTokenUsername();
    return name ? `sso: ${name}` : "sso";
  }
  return "";
}

const NAV: NavItem[] = [
  { to: "/overview", label: "Overview" },
  { to: "/pools", label: "Pools" },
  { to: "/clients", label: "Clients" },
  { to: "/apps", label: "Apps" },
  // Caches exposes prepared-statement texts; logs leak SQL through the
  // operator stream. Both are personal-data paths and only Sso/Admin
  // roles can fetch them — hide the links for anonymous viewers.
  { to: "/caches", label: "Caches", personal: true },
  { to: "/logs", label: "Logs", personal: true },
  { to: "/config", label: "Config" },
  // War room (/wall) intentionally omitted from the sidebar. It is a
  // kiosk view of the same Overview data, reached from the Overview
  // hero ("Open war room" button). Surfacing it as a top-level link
  // duplicated the operator's mental model and pushed the nav past
  // 7±2 entries.
];

// 3 s tick: signals visible on every page, not incident-grade. Backend
// /api/overview cache (250 ms TTL) absorbs the overlap with the active
// page's faster poll on Overview/Wall.
const SIDEBAR_POLL_MS = 3000;

interface RateState {
  qps: number;
  errsPerSec: number;
}

interface PrevTotals {
  ts: number;
  queries: number;
  errors: number;
}

const PREV_TOTALS_KEY = "pgdoorman.prev.totals";

function loadPrevTotals(): PrevTotals | null {
  try {
    const raw = localStorage.getItem(PREV_TOTALS_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<PrevTotals>;
    if (
      typeof parsed.ts !== "number" ||
      typeof parsed.queries !== "number" ||
      typeof parsed.errors !== "number"
    ) {
      return null;
    }
    return parsed as PrevTotals;
  } catch {
    return null;
  }
}

function savePrevTotals(v: PrevTotals) {
  try {
    localStorage.setItem(PREV_TOTALS_KEY, JSON.stringify(v));
  } catch {
    /* private mode / quota — no-op. */
  }
}

export function Sidebar() {
  const { authHeader, basic, setBasic, ssoToken, setSsoToken, role } =
    useAdminAuth();
  const location = useLocation();

  const [version, setVersion] = useState<string | null>(null);
  const [overview, setOverview] = useState<OverviewDto | null>(null);
  const [pools, setPools] = useState<PoolsDto | null>(null);
  const [proc, setProc] = useState<ProcessDto | null>(null);
  const [rate, setRate] = useState<RateState>({ qps: 0, errsPerSec: 0 });
  // Seed prev snapshot from localStorage so the first poll after a page
  // navigation immediately yields a delta — without this the sidebar
  // (and the operator) saw "0.00 / 0.00" every time they reopened the
  // tab or jumped between pages.
  const prevRef = useRef<PrevTotals | null>(loadPrevTotals());

  useEffect(() => {
    let cancelled = false;
    apiGet<VersionDto>("/api/version", authHeader)
      .then((d) => {
        if (!cancelled) setVersion(d.version);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [authHeader]);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      apiGet<OverviewDto>("/api/overview", authHeader)
        .then((d) => {
          if (cancelled) return;
          setOverview(d);
          const prev = prevRef.current;
          const cur: PrevTotals = {
            ts: d.ts,
            queries: d.query_count_total,
            errors: d.errors_count_total,
          };
          if (prev) {
            const dt = (cur.ts - prev.ts) / 1000;
            // Sanity: only use the persisted prev when it's within the
            // last 60 s. A stale snapshot (laptop slept, tab closed for
            // hours) would compute a meaningless rate.
            if (dt > 0 && dt < 60) {
              setRate({
                qps: Math.max(0, (cur.queries - prev.queries) / dt),
                errsPerSec: Math.max(0, (cur.errors - prev.errors) / dt),
              });
            }
          }
          prevRef.current = cur;
          savePrevTotals(cur);
        })
        .catch(() => {});
      apiGet<PoolsDto>("/api/pools", authHeader)
        .then((d) => {
          if (!cancelled) setPools(d);
        })
        .catch(() => {});
      apiGet<ProcessDto>("/api/process", authHeader)
        .then((d) => {
          if (!cancelled) setProc(d);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, SIDEBAR_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [authHeader]);

  const health = useMemo(() => {
    if (!overview || !pools) return null;
    // The sidebar is an ambient indicator; pool history for the threshold
    // engine lives on Overview. Pass an empty map so only instantaneous
    // signals (saturation, query p95, wait) feed the verdict — that
    // matches what an operator looking at the chip-strip expects.
    return aggregateHealth(overview, pools.pools, new Map(), null);
  }, [overview, pools]);

  const alertCounts = useMemo(() => {
    if (!health) return { crit: 0, deg: 0 };
    let crit = 0;
    let deg = 0;
    for (const p of health.perPool) {
      if (p.severity === "critical") crit += 1;
      else if (p.severity === "degraded") deg += 1;
    }
    return { crit, deg };
  }, [health]);

  const maxSat = useMemo(() => {
    if (!pools) return null;
    let m = 0;
    for (const p of pools.pools) {
      if (p.max_connections > 0) {
        const s = p.active / p.max_connections;
        if (s > m) m = s;
      }
    }
    return m * 100;
  }, [pools]);

  const hottest: HottestDatabaseDto | null = overview?.hottest_database ?? null;

  // Kiosk mode: the war-room route hides every piece of chrome (sidebar
  // included) so a wall display has nothing but signals. Operators reach
  // /wall by clicking the nav link, the page itself renders a back-out
  // affordance.
  if (location.pathname === "/wall") return null;

  return (
    <nav className="sticky top-0 flex h-screen w-60 shrink-0 flex-col border-r border-border bg-surface">
      <div className="border-b border-border px-4 py-3">
        <Link
          to="/overview"
          className="font-mono text-sm font-medium leading-none text-text hover:text-accent"
          aria-label="pg_doorman home"
        >
          pg_doorman
        </Link>
        {version && (
          <div className="mt-1 font-mono text-[10px] text-text-dim">v{version}</div>
        )}
      </div>

      <SignalsBlock
        health={health}
        rate={rate}
        alertCounts={alertCounts}
        maxSat={maxSat}
        hottest={hottest}
      />

      <ul className="flex-1 px-2 pb-3 pt-3">
        {NAV.filter((item) => !item.personal || role !== "anonymous").map(
          (item) => (
            <li key={item.to}>
              <NavLink
                to={item.to}
                className={({ isActive }) =>
                  `block border-l-2 px-4 py-2 text-sm font-semibold transition-colors ${
                    isActive
                      ? "border-accent bg-accent/10 text-text"
                      : "border-transparent text-text-muted hover:border-border-strong hover:text-text"
                  }`
                }
              >
                {item.label}
              </NavLink>
            </li>
          ),
        )}
      </ul>

      {proc && (
        <div className="space-y-1 border-t border-border px-4 py-3 font-mono text-[11px] text-text-dim">
          <div className="truncate" title={proc.hostname}>
            {proc.hostname || "host"}
          </div>
          <div>
            pid {proc.pid} · up {fmtUptime(proc.uptime_seconds)}
          </div>
        </div>
      )}

      <div className="space-y-2 border-t border-border px-4 py-3 text-xs text-text-dim">
        {(basic || ssoToken) && (
          <button
            type="button"
            onClick={() => {
              setBasic(null, false);
              setSsoToken(null);
            }}
            className="font-mono uppercase tracking-wider text-text-muted hover:text-accent"
            title="Click to clear stored credentials and re-prompt."
          >
            sign out ({signedInLabel(basic, ssoToken)})
          </button>
        )}
      </div>
    </nav>
  );
}

function SignalsBlock({
  health,
  rate,
  alertCounts,
  maxSat,
  hottest,
}: {
  health: { state: "ok" | "degraded" | "critical" } | null;
  rate: RateState;
  alertCounts: { crit: number; deg: number };
  maxSat: number | null;
  hottest: HottestDatabaseDto | null;
}) {
  const dotClass =
    health?.state === "critical"
      ? "bg-danger animate-pulse"
      : health?.state === "degraded"
        ? "bg-warning"
        : "bg-success";
  const dotLabel =
    health?.state === "critical"
      ? "CRITICAL"
      : health?.state === "degraded"
        ? "DEGRADED"
        : health
          ? "OK"
          : "…";
  const satTone =
    maxSat === null
      ? "text-text-dim"
      : maxSat >= 90
        ? "text-danger"
        : maxSat >= 70
          ? "text-warning"
          : "text-text";
  return (
    <div className="space-y-3 border-b border-border px-4 py-3">
      <div className="flex items-center gap-2">
        <span className={`h-2 w-2 rounded-full ${dotClass}`} aria-hidden="true" />
        <span className="text-xs font-semibold uppercase tracking-wider text-text">
          {dotLabel}
        </span>
      </div>

      <div className="grid grid-cols-2 gap-x-3 gap-y-1.5">
        <SignalRow
          label="qps"
          value={fmtRate(rate.qps)}
          tone="text-text"
        />
        <SignalRow
          label="err/s"
          value={fmtRate(rate.errsPerSec)}
          tone={
            rate.errsPerSec > 10
              ? "text-danger"
              : rate.errsPerSec > 1
                ? "text-warning"
                : "text-text"
          }
        />
        <SignalRow
          label="sat max"
          value={maxSat === null ? "—" : `${maxSat.toFixed(0)}%`}
          tone={satTone}
        />
        <SignalRow
          label="alerts"
          value={
            alertCounts.crit === 0 && alertCounts.deg === 0
              ? "0"
              : `${alertCounts.crit}c·${alertCounts.deg}d`
          }
          tone={
            alertCounts.crit > 0
              ? "text-danger"
              : alertCounts.deg > 0
                ? "text-warning"
                : "text-text"
          }
        />
      </div>

      {hottest && (
        <div className="border-t border-border/60 pt-2 text-[11px] text-text-dim">
          <div className="text-[10px] uppercase tracking-wider">hottest db</div>
          <div
            className="mt-0.5 truncate font-mono text-text"
            title={hottest.name}
          >
            {hottest.name}
          </div>
          <div className="font-mono">
            {hottest.active_connections} active · {hottest.total_connections} total
          </div>
        </div>
      )}
    </div>
  );
}

function SignalRow({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone: string;
}) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-[10px] uppercase tracking-wider text-text-dim">
        {label}
      </span>
      <span className={`font-mono text-xs font-semibold tabular ${tone}`}>
        {value}
      </span>
    </div>
  );
}

function fmtRate(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${(n / 1000).toFixed(0)}k`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  if (n >= 10) return n.toFixed(0);
  return n.toFixed(2);
}

function fmtUptime(s: number): string {
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  const d = Math.floor(h / 24);
  return `${d}d ${h % 24}h`;
}
