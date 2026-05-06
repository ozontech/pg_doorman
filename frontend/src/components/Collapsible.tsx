import { useEffect, useState, type ReactNode } from "react";

interface CollapsibleProps {
  /** Stable key — open state persisted under `pgdoorman.collapse.${id}`. */
  id: string;
  title: string;
  defaultOpen?: boolean;
  children: ReactNode;
}

/** Section header that toggles its body. Open state survives a tab refresh. */
export function Collapsible({ id, title, defaultOpen = false, children }: CollapsibleProps) {
  const storageKey = `pgdoorman.collapse.${id}`;
  const [open, setOpen] = useState<boolean>(() => {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw === "1") return true;
      if (raw === "0") return false;
    } catch {
      /* private mode — fall through to default. */
    }
    return defaultOpen;
  });

  useEffect(() => {
    try {
      localStorage.setItem(storageKey, open ? "1" : "0");
    } catch {
      /* private mode — no-op. */
    }
  }, [open, storageKey]);

  return (
    <section className="border-b border-border">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 px-4 py-2 text-left text-xs uppercase tracking-wide text-text-muted hover:bg-surface-2"
      >
        <span aria-hidden>{open ? "▾" : "▸"}</span>
        <span>{title}</span>
      </button>
      {open && children}
    </section>
  );
}
