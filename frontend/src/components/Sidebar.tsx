import { NavLink } from "react-router-dom";

const NAV = [
  { to: "/overview", label: "Overview" },
  { to: "/pools", label: "Pools" },
  { to: "/clients", label: "Clients" },
  { to: "/caches", label: "Caches" },
  { to: "/logs", label: "Logs" },
  { to: "/config", label: "Config" },
];

export function Sidebar() {
  return (
    <nav className="flex h-screen w-48 flex-col border-r border-border bg-surface">
      <div className="px-4 py-4 text-md font-semibold text-text">pg_doorman</div>
      <ul className="flex-1">
        {NAV.map((item) => (
          <li key={item.to}>
            <NavLink
              to={item.to}
              className={({ isActive }) =>
                `block px-4 py-2 text-sm ${
                  isActive
                    ? "bg-surface-2 text-accent border-l-2 border-accent"
                    : "text-text-muted hover:bg-surface-2 hover:text-text"
                }`
              }
            >
              {item.label}
            </NavLink>
          </li>
        ))}
      </ul>
      <div className="px-4 py-3 text-xs text-text-dim border-t border-border">
        phase 5 skeleton
      </div>
    </nav>
  );
}
