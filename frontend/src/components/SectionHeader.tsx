import type { ReactNode } from "react";

interface SectionHeaderProps {
  title: string;
  /** One-line description: what is rendered. */
  what: string;
  /** One-line description: cadence and source. */
  how?: string;
  /** One-line description: where the threshold is, what counts as healthy. */
  normal?: string;
  /** Optional right-aligned slot (status pill, last-updated chip, etc.). */
  right?: ReactNode;
}

/**
 * Header + help block reused above every chart, table, and panel. Keeps the
 * "what / how / normal" answer one glance away so an operator doesn't have
 * to read source code or the spec to interpret a number.
 */
export function SectionHeader({ title, what, how, normal, right }: SectionHeaderProps) {
  return (
    <header className="border-b border-border bg-surface px-6 pb-3 pt-4">
      <div className="flex items-baseline gap-3">
        <h2 className="font-mono text-[11px] uppercase tracking-[0.22em] text-text">
          {title}
        </h2>
        <span aria-hidden className="h-px flex-1 bg-border" />
        {right}
      </div>
      <p className="mt-2 max-w-3xl text-xs leading-relaxed text-text-muted">
        <span className="text-text">{what}</span>
        {how && <> · <span className="text-text-dim">{how}</span></>}
        {normal && <> · <span className="text-text-dim">{normal}</span></>}
      </p>
    </header>
  );
}
