import { Monitor, Moon, Sun } from "lucide-react";
import { useTheme, type ThemePref } from "../hooks/useTheme";

// Compact three-way toggle: Light / System / Dark. Lives in the sidebar
// footer so the choice is reachable but does not compete with operational
// signals for attention. Active segment carries the accent colour; the
// others stay muted so the row reads as a single control.
const OPTIONS: { value: ThemePref; label: string; Icon: typeof Sun }[] = [
  { value: "light", label: "Light theme", Icon: Sun },
  { value: "system", label: "Match system", Icon: Monitor },
  { value: "dark", label: "Dark theme", Icon: Moon },
];

export function ThemeToggle() {
  const { pref, setPref } = useTheme();
  return (
    <div
      role="radiogroup"
      aria-label="Colour theme"
      className="inline-flex items-center overflow-hidden rounded-md border border-border bg-surface-2 p-0.5"
    >
      {OPTIONS.map(({ value, label, Icon }) => {
        const active = pref === value;
        return (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={active}
            aria-label={label}
            onClick={() => setPref(value)}
            className={`rounded p-1.5 transition-colors ${
              active
                ? "bg-accent text-accent-fg"
                : "text-text-muted hover:text-text"
            }`}
          >
            <Icon size={16} strokeWidth={1.75} />
          </button>
        );
      })}
    </div>
  );
}
