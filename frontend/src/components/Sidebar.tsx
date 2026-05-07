import { useEffect, useState } from "react";
import { NavLink } from "react-router-dom";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import { getSsoTokenUsername } from "../lib/jwt";
import type { HottestDatabaseDto, OverviewDto, VersionDto } from "../types";

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
  { to: "/wall", label: "War room" },
];

export function Sidebar() {
  const { authHeader, basic, setBasic, ssoToken, setSsoToken, role } =
    useAdminAuth();
  const [version, setVersion] = useState<string | null>(null);
  const [hottest, setHottest] = useState<HottestDatabaseDto | null>(null);
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
    // 5 s tick is loose on purpose — the value is for ambient awareness
    // ("which DB is hot right now"), not incident-grade tracking. The
    // Overview and Wall pages already poll /api/overview at 1.5 s when
    // active, and the backend snapshot cache (250 ms TTL) absorbs any
    // overlap with the sidebar's independent tick on other pages.
    const tick = () => {
      apiGet<OverviewDto>("/api/overview", authHeader)
        .then((d) => {
          if (!cancelled) setHottest(d.hottest_database ?? null);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [authHeader]);
  return (
    <nav className="sticky top-0 flex h-screen w-60 shrink-0 flex-col border-r border-border bg-surface">
      <div className="px-6 py-7">
        <div className="text-xs font-medium uppercase tracking-[0.2em] text-text-dim">
          pg_doorman
        </div>
        <div className="mt-1.5 text-base font-semibold text-text">Admin console</div>
        {hottest && (
          <div
            className="mt-3 border-t border-border pt-3 text-xs text-text-dim"
            title="Database holding the most live backend connections right now"
          >
            <div className="text-[10px] uppercase tracking-wider">hottest db</div>
            <div className="mt-0.5 truncate font-mono text-text" title={hottest.name}>
              {hottest.name}
            </div>
            <div className="font-mono">
              {hottest.active_connections} active · {hottest.total_connections} total
            </div>
          </div>
        )}
      </div>
      <ul className="flex-1 px-2 pb-3">
        {NAV.filter((item) => !item.personal || role !== "anonymous").map(
          (item) => (
            <li key={item.to}>
              <NavLink
                to={item.to}
                className={({ isActive }) =>
                  `block rounded-md px-4 py-2 text-sm font-medium transition-colors ${
                    isActive
                      ? "bg-surface-2 text-text"
                      : "text-text-muted hover:bg-surface-2/60 hover:text-text"
                  }`
                }
              >
                {item.label}
              </NavLink>
            </li>
          ),
        )}
      </ul>
      <div className="space-y-2 border-t border-border px-6 py-4 text-xs text-text-dim">
        {(basic || ssoToken) && (
          // Visible sign-out so an operator who ticked "Remember me on
          // this device" can wipe the localStorage entries without diving
          // into browser dev-tools. Re-arms the AuthGate modal on the
          // next request.
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
        <div>{version ? `v${version}` : "—"}</div>
      </div>
    </nav>
  );
}
