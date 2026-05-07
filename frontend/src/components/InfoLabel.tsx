import type { ReactNode } from "react";

/**
 * Operator-facing tooltip label. Wraps text with a cursor-help affordance
 * and a styled popover so an operator new to pg_doorman knows the value
 * has an explanation behind it. Replaces native `title=` attributes which
 * (a) render in the browser's chrome with no relation to the UI palette
 * and (b) give no cursor cue that hover means anything.
 *
 * The popover is CSS-only: a sibling span that becomes visible on group-
 * hover. No portal, no JS positioning — fits everywhere KV/header rows
 * already render and never escapes the tile boundary in unfortunate
 * directions because we let it grow upward by default.
 */
export function InfoLabel({
  tip,
  children,
  className = "",
}: {
  tip?: string;
  children: ReactNode;
  className?: string;
}) {
  if (!tip) return <span className={className}>{children}</span>;
  return (
    <span className={`group relative inline-flex cursor-help items-baseline ${className}`}>
      {children}
      <span
        aria-hidden="true"
        className="ml-1 select-none text-[10px] leading-none text-text-dim group-hover:text-accent"
      >
        ⓘ
      </span>
      <span
        role="tooltip"
        className="
          pointer-events-none invisible absolute bottom-full left-0 z-30 mb-2
          w-72 max-w-[20rem] border border-border-strong bg-surface px-3 py-2
          text-left text-xs leading-snug text-text shadow-xl break-words
          opacity-0 transition-opacity duration-100
          group-hover:visible group-hover:opacity-100
        "
      >
        {tip}
      </span>
    </span>
  );
}
