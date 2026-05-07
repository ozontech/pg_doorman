import { useEffect, useState } from "react";
import { NavLink } from "react-router-dom";
import { apiGet } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import type { VersionDto } from "../types";

const NAV: { to: string; label: string }[] = [
  { to: "/overview", label: "Overview" },
  { to: "/pools", label: "Pools" },
  { to: "/clients", label: "Clients" },
  { to: "/apps", label: "Apps" },
  { to: "/caches", label: "Caches" },
  { to: "/logs", label: "Logs" },
  { to: "/config", label: "Config" },
  { to: "/wall", label: "War room" },
];

export function Sidebar() {
  const { authHeader, creds, setCreds } = useAdminAuth();
  const [version, setVersion] = useState<string | null>(null);
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
  return (
    <nav className="sticky top-0 flex h-screen w-60 shrink-0 flex-col border-r border-border bg-surface">
      <div className="px-6 py-7">
        <div className="text-xs font-medium uppercase tracking-[0.2em] text-text-dim">
          pg_doorman
        </div>
        <div className="mt-1.5 text-base font-semibold text-text">Admin console</div>
      </div>
      <ul className="flex-1 px-2 pb-3">
        {NAV.map((item) => (
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
        ))}
      </ul>
      <div className="space-y-2 border-t border-border px-6 py-4 text-xs text-text-dim">
        {creds && (
          // Visible sign-out so an operator who ticked "Remember me on
          // this device" can wipe the localStorage entry without diving
          // into browser dev-tools. Re-arms the AuthGate modal on the
          // next request.
          <button
            type="button"
            onClick={() => setCreds(null, false)}
            className="font-mono uppercase tracking-wider text-text-muted hover:text-accent"
            title={`Signed in as ${creds.username}. Click to clear stored credentials and re-prompt.`}
          >
            sign out ({creds.username})
          </button>
        )}
        <div>{version ? `v${version}` : "—"}</div>
      </div>
    </nav>
  );
}
