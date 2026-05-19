import { useEffect, useMemo, useRef, useState } from "react";
import { Link, NavLink, useLocation } from "react-router-dom";
import {
  AppWindow,
  BookOpen,
  Boxes,
  Database,
  ExternalLink,
  LayoutDashboard,
  ScrollText,
  Server,
  Settings,
  Users,
  type LucideIcon,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { useProcessIdentityToast } from "../hooks/useProcessIdentity";
import { fmtRate, fmtUptime } from "../lib/format";
import { getSsoTokenUsername } from "../lib/jwt";
import { ThemeToggle } from "./ThemeToggle";
import { aggregateHealth } from "../lib/thresholds";
import type {
  HottestDatabaseDto,
  OverviewDto,
  PoolsDto,
  ProcessDto,
  VersionDto,
} from "../types";

type NavItem = { to: string; label: string; icon: LucideIcon; personal?: boolean };

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
  { to: "/overview", label: "Overview", icon: LayoutDashboard },
  { to: "/pools", label: "Pools", icon: Database },
  { to: "/clients", label: "Clients", icon: Users },
  { to: "/servers", label: "Servers", icon: Server },
  { to: "/apps", label: "Apps", icon: AppWindow },
  // Caches and logs expose SQL text, so anonymous viewers do not get links.
  { to: "/caches", label: "Caches", icon: Boxes, personal: true },
  { to: "/logs", label: "Logs", icon: ScrollText, personal: true },
  { to: "/config", label: "Config", icon: Settings },
  // War room (/wall) is reached from Overview because it is the same data
  // in a large-screen layout.
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

// Host-scoped so two pooler tabs (pooler-a / pooler-b) keep separate
// previous-totals slots — without this they would overwrite each other.
const PREV_TOTALS_KEY = `pgdoorman.prev.totals.${
  typeof window !== "undefined" ? window.location.host : "any"
}`;

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
  // Seed prev snapshot from localStorage so the first poll after a page
  // navigation immediately yields a delta — without this the sidebar
  // (and the operator) saw "0.00 / 0.00" every time they reopened the
  // tab or jumped between pages.
  const prevRef = useRef<PrevTotals | null>(loadPrevTotals());
  const [rate, setRate] = useState<RateState>({ qps: 0, errsPerSec: 0 });

  // Version is one-shot — render once and forget. No refetch interval.
  const versionQuery = useQuery({
    queryKey: ["sidebar.version", authHeader],
    queryFn: ({ signal }) =>
      apiGet<VersionDto>("/api/version", authHeader, signal),
    staleTime: 5 * 60_000,
  });
  const version = versionQuery.data?.version ?? null;

  // Operational polls. TanStack Query keeps the last response in cache
  // across page navigations so reopening the SPA does not start the
  // sidebar from "0.00" while the first request hits the wire.
  const overviewQuery = useQuery({
    queryKey: ["sidebar.overview", authHeader],
    queryFn: ({ signal }) =>
      apiGet<OverviewDto>("/api/overview", authHeader, signal),
    refetchInterval: SIDEBAR_POLL_MS,
  });
  const overview = overviewQuery.data ?? null;

  const poolsQuery = useQuery({
    queryKey: ["sidebar.pools", authHeader],
    queryFn: ({ signal }) =>
      apiGet<PoolsDto>("/api/pools", authHeader, signal),
    refetchInterval: SIDEBAR_POLL_MS,
  });
  const pools = poolsQuery.data ?? null;

  const procQuery = useQuery({
    queryKey: ["sidebar.process", authHeader],
    queryFn: ({ signal }) =>
      apiGet<ProcessDto>("/api/process", authHeader, signal),
    refetchInterval: SIDEBAR_POLL_MS,
  });
  const proc = procQuery.data ?? null;

  // Derive QPS / errors-per-second from the previous snapshot whenever
  // /api/overview returns. Persisted prevRef survives mounts so the
  // very first response after a page change immediately yields a rate.
  //
  // Counter rollback alone is NOT a restart signal: RELOAD and dynamic
  // pool GC drop pools from `pool_lookup`, which is what the backend
  // sums to produce query_count_total — so totals legitimately fall
  // without the process going anywhere. Real restart detection lives
  // in useProcessIdentity() and is fed by pid + started_at_ms.
  useEffect(() => {
    if (!overview) return;
    const cur: PrevTotals = {
      ts: overview.ts,
      queries: overview.query_count_total,
      errors: overview.errors_count_total,
    };
    const prev = prevRef.current;
    if (prev && prev.ts !== cur.ts) {
      const dt = (cur.ts - prev.ts) / 1000;
      const counterReset =
        cur.queries < prev.queries || cur.errors < prev.errors;
      if (!counterReset && dt > 0 && dt < 60) {
        setRate({
          qps: Math.max(0, (cur.queries - prev.queries) / dt),
          errsPerSec: Math.max(0, (cur.errors - prev.errors) / dt),
        });
      }
      // counterReset = drop a tick rather than show a fake spike; the
      // next /api/overview poll establishes a fresh baseline against
      // the post-RELOAD pool set.
    }
    prevRef.current = cur;
    savePrevTotals(cur);
  }, [overview]);

  // Identity-based restart detection — toast once per real restart, where
  // "real" means pid or started_at_ms moved. Counter behaviour is
  // ignored here on purpose.
  useProcessIdentityToast(overview);

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

  // Wall mode hides the sidebar; the page renders its own back control.
  if (location.pathname === "/wall") return null;

  return (
    <nav className="sticky top-0 hidden h-screen w-64 shrink-0 flex-col border-r border-border bg-surface md:flex">
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
          (item) => {
            const Icon = item.icon;
            return (
              <li key={item.to}>
                <NavLink
                  to={item.to}
                  className={({ isActive }) =>
                    `flex items-center gap-3 border-l-2 px-4 py-2.5 text-sm font-medium transition-colors ${
                      isActive
                        ? "border-accent bg-accent/10 text-text"
                        : "border-transparent text-text-muted hover:border-border-strong hover:text-text"
                    }`
                  }
                >
                  <Icon size={18} strokeWidth={1.75} aria-hidden="true" />
                  <span>{item.label}</span>
                </NavLink>
              </li>
            );
          },
        )}
      </ul>

      {proc && (
        <div className="space-y-1 border-t border-border px-4 py-3 font-mono text-xs text-text-dim">
          <div className="truncate" title={proc.hostname}>
            {proc.hostname || "host"}
          </div>
          <div>
            pid {proc.pid} · up {fmtUptime(proc.uptime_seconds)}
          </div>
        </div>
      )}

      <div className="space-y-2 border-t border-border px-4 py-3 text-sm text-text-dim">
        <TransportLine />
        <a
          href="https://ozontech.github.io/pg_doorman/"
          target="_blank"
          rel="noreferrer noopener"
          className="flex items-center gap-2 text-text-muted hover:text-accent"
          title="Open pg_doorman documentation in a new tab"
        >
          <BookOpen size={14} strokeWidth={1.75} aria-hidden="true" />
          <span>Documentation</span>
          <ExternalLink size={12} strokeWidth={1.75} aria-hidden="true" className="ml-auto" />
        </a>
        {(basic || ssoToken) ? (
          <button
            type="button"
            onClick={() => {
              setBasic(null, false);
              setSsoToken(null);
            }}
            className="block w-full truncate text-left text-text-muted hover:text-accent"
            title={`Sign out · ${signedInLabel(basic, ssoToken)} — click to clear stored credentials and re-prompt.`}
          >
            sign out · {signedInLabel(basic, ssoToken)}
          </button>
        ) : (
          <span className="block text-text-dim">anonymous</span>
        )}
        <div className="flex items-center justify-between">
          <span className="text-xs text-text-dim">theme</span>
          <ThemeToggle />
        </div>
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
        {/* Live pulse: a single dot for the verdict plus a soft ping ring
            around it so the operator can tell at a glance that data is
            still flowing. Pulse animation is the same regardless of
            severity; the dot colour carries the verdict. */}
        <span className="relative inline-flex h-2 w-2" aria-hidden="true">
          <span
            className={`absolute inline-flex h-full w-full animate-ping rounded-full opacity-60 ${
              health?.state === "critical"
                ? "bg-danger"
                : health?.state === "degraded"
                  ? "bg-warning"
                  : "bg-success"
            }`}
          />
          <span className={`relative inline-flex h-2 w-2 rounded-full ${dotClass}`} />
        </span>
        <span className="text-xs font-semibold text-text">{dotLabel}</span>
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
          <div className="text-[10px] uppercase tracking-wider">busiest db</div>
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

// Persistent transport indicator in the sidebar footer.
function TransportLine() {
  const protocol =
    typeof window !== "undefined" ? window.location.protocol : "";
  const secure = protocol === "https:";
  return (
    <div
      className={`flex items-center gap-2 text-xs ${
        secure ? "text-text-dim" : "text-warning"
      }`}
      title={
        secure
          ? "Connection is HTTPS; credentials and admin actions are encrypted in transit."
          : "Connection is plain HTTP; credentials and admin actions are sent in the clear. Use HTTPS in production."
      }
    >
      <span
        aria-hidden="true"
        className={`h-1.5 w-1.5 rounded-full ${secure ? "bg-success" : "bg-warning"}`}
      />
      transport · {secure ? "https" : "http"}
    </div>
  );
}

// fmtRate / fmtUptime were duplicated here; both moved to lib/format.
