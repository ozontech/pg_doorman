import { NavLink } from "react-router-dom";

const NAV: { to: string; label: string }[] = [
  { to: "/overview", label: "Overview" },
  { to: "/pools", label: "Pools" },
  { to: "/clients", label: "Clients" },
  { to: "/apps", label: "Apps" },
  { to: "/caches", label: "Caches" },
  { to: "/logs", label: "Logs" },
  { to: "/config", label: "Config" },
];

export function Sidebar() {
  return (
    <nav className="flex h-screen w-60 shrink-0 flex-col border-r border-border bg-surface">
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
      <div className="border-t border-border px-6 py-4 text-xs text-text-dim">
        v3.7.0 · feat/web-ui
      </div>
    </nav>
  );
}
