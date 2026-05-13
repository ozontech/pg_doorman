import { useEffect, useState, type ReactNode } from "react";

interface Shortcut {
  keys: string[];
  description: string;
}

const SHORTCUTS: { group: string; items: Shortcut[] }[] = [
  {
    group: "Navigation",
    items: [
      { keys: ["⌘", "K"], description: "Open command palette" },
      { keys: ["⌃", "K"], description: "Same on non-Mac" },
      { keys: ["?"], description: "Show keyboard shortcuts" },
      { keys: ["Esc"], description: "Close popovers or exit war room" },
    ],
  },
  {
    group: "War room",
    items: [
      { keys: ["Esc"], description: "Return to console from /wall" },
    ],
  },
];

// Global help dialog. Listens for `?` outside any input/textarea and
// opens the modal; `Esc` closes it. Lives alongside CommandPalette in
// App.tsx so the keymap is global without each page wiring its own
// listener.
export function HelpModal() {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && open) {
        setOpen(false);
        return;
      }
      if (e.key !== "?") return;
      const target = e.target as HTMLElement | null;
      // Ignore the shortcut when the operator is typing into a field —
      // a filter input wants the literal `?` character.
      const tag = target?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) {
        return;
      }
      e.preventDefault();
      setOpen(true);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open]);

  if (!open) return null;
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="help-modal-title"
      className="fixed inset-0 z-50 flex items-start justify-center bg-bg/70 pt-[15vh] backdrop-blur-sm"
      onClick={() => setOpen(false)}
    >
      <div
        className="w-[min(520px,calc(100vw-2rem))] overflow-hidden rounded-lg border border-border-strong bg-surface text-text shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="border-b border-border px-5 pt-5 pb-4">
          <h2 id="help-modal-title" className="text-lg font-semibold tracking-tight">
            Keyboard shortcuts
          </h2>
          <p className="mt-1 text-sm text-text-muted">
            Shortcuts work anywhere outside text fields. Press{" "}
            <Kbd>Esc</Kbd> to close.
          </p>
        </header>
        <div className="space-y-4 px-5 py-4">
          {SHORTCUTS.map((group) => (
            <section key={group.group}>
              <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-text-dim">
                {group.group}
              </h3>
              <ul className="space-y-1.5">
                {group.items.map((sc, i) => (
                  <li
                    key={i}
                    className="flex items-center justify-between gap-3 text-sm"
                  >
                    <span className="text-text">{sc.description}</span>
                    <span className="flex shrink-0 items-center gap-1">
                      {sc.keys.map((k) => (
                        <Kbd key={k}>{k}</Kbd>
                      ))}
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}

function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-6 min-w-[1.5rem] items-center justify-center rounded-md border border-border-strong bg-surface-2 px-1.5 font-mono text-xs text-text shadow-sm">
      {children}
    </kbd>
  );
}
