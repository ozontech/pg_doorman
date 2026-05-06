import { useEffect, type ReactNode } from "react";

interface DrawerProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
}

/**
 * Right-side overlay drawer. Click outside or press Escape to close. Used
 * for per-row detail panels (pool drawer in /pools, etc.) that would push
 * the layout around if rendered inline.
 */
export function Drawer({ open, title, onClose, children }: DrawerProps) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-40 flex justify-end">
      <button
        type="button"
        aria-label="Close drawer"
        onClick={onClose}
        className="flex-1 bg-bg/60 backdrop-blur-sm"
      />
      <aside className="w-[28rem] max-w-full overflow-y-auto border-l border-border bg-surface shadow-2xl">
        <header className="flex items-center justify-between border-b border-border px-6 py-4">
          <h2 className="font-mono text-sm font-semibold text-text">{title}</h2>
          <button
            type="button"
            onClick={onClose}
            className="text-text-dim hover:text-text"
            aria-label="Close"
          >
            ✕
          </button>
        </header>
        <div className="p-6">{children}</div>
      </aside>
    </div>
  );
}
