import { NavLink } from "react-router-dom";

const NAV: { to: string; label: string; index: string }[] = [
  { to: "/overview", label: "Overview", index: "01" },
  { to: "/pools", label: "Pools", index: "02" },
  { to: "/clients", label: "Clients", index: "03" },
  { to: "/caches", label: "Caches", index: "04" },
  { to: "/logs", label: "Logs", index: "05" },
  { to: "/config", label: "Config", index: "06" },
];

export function Sidebar() {
  return (
    <nav className="flex h-screen w-56 shrink-0 flex-col border-r border-border bg-surface">
      <div className="border-b border-border px-5 py-5">
        <div className="font-mono text-[10px] uppercase tracking-[0.22em] text-text-dim">
          pooler
        </div>
        <div className="mt-1 font-mono text-md font-semibold text-text">pg_doorman</div>
        <div className="mt-1 font-mono text-[10px] uppercase tracking-wide text-accent">
          ◆ admin console
        </div>
      </div>
      <ul className="flex-1 py-3">
        {NAV.map((item) => (
          <li key={item.to}>
            <NavLink
              to={item.to}
              className={({ isActive }) =>
                `flex items-center gap-3 px-5 py-2 text-sm transition-colors ${
                  isActive
                    ? "bg-surface-2 text-text"
                    : "text-text-muted hover:bg-surface-2 hover:text-text"
                }`
              }
            >
              {({ isActive }) => (
                <>
                  <span
                    aria-hidden
                    className={`font-mono text-[10px] tabular ${
                      isActive ? "text-accent" : "text-text-dim"
                    }`}
                  >
                    {item.index}
                  </span>
                  <span className="flex-1 font-medium">{item.label}</span>
                  {isActive && <span aria-hidden className="h-1.5 w-1.5 bg-accent" />}
                </>
              )}
            </NavLink>
          </li>
        ))}
      </ul>
      <div className="border-t border-border px-5 py-3 font-mono text-[10px] uppercase tracking-wide text-text-dim">
        v3.7 · feat/web-ui
      </div>
    </nav>
  );
}
