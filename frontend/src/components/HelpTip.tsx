import { useEffect, useRef, useState, type ReactNode } from "react";

interface HelpTipProps {
  title: string;
  children: ReactNode;
}

/**
 * Small circular "i" trigger that opens a popover with longer-form help next
 * to a section title. Click to toggle, click outside or press Escape to
 * close. Hover triggers focus too so a quick mouse-over previews without a
 * commit. Anchored to the trigger; the popover renders to the right by
 * default, drifting left on small viewports.
 */
export function HelpTip({ title, children }: HelpTipProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onClick = (e: MouseEvent) => {
      if (!ref.current) return;
      if (!ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative inline-flex">
      <button
        type="button"
        aria-expanded={open}
        aria-label={`Help: ${title}`}
        onClick={() => setOpen((v) => !v)}
        onMouseEnter={() => setOpen(true)}
        className="grid h-5 w-5 place-items-center rounded-full border border-border-strong text-[10px] font-semibold text-text-muted transition-colors hover:border-accent hover:text-accent"
      >
        i
      </button>
      {open && (
        <div className="absolute left-0 top-7 z-30">
          <div className="max-h-[60vh] w-80 max-w-[calc(100vw-2rem)] overflow-y-auto rounded-md border border-border-strong bg-surface-3 p-4 shadow-xl">
            <div className="mb-2 text-[10px] font-semibold uppercase tracking-[0.2em] text-accent">
              {title}
            </div>
            <div className="space-y-2 text-sm leading-relaxed text-text">{children}</div>
          </div>
        </div>
      )}
    </div>
  );
}
