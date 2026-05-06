import type { ReactNode } from "react";
import { HelpTip } from "./HelpTip";

interface SectionHeaderProps {
  title: string;
  /** What is rendered in the section. Shown as the first line in the popover. */
  what?: string;
  /** How / where the data comes from and how often it refreshes. */
  how?: string;
  /** Threshold or healthy-range note. */
  normal?: string;
  /** Optional right-aligned slot (status pill, last-updated chip, button). */
  right?: ReactNode;
}

/**
 * Section header used above every chart, table, and panel. Shows just the
 * title; full guidance lives in a popover behind the "i" icon so the layout
 * stays clean. The popover answers three questions: what is on screen, how
 * the numbers update, and what counts as healthy.
 */
export function SectionHeader({ title, what, how, normal, right }: SectionHeaderProps) {
  const hasHelp = Boolean(what || how || normal);
  return (
    <header className="flex items-center gap-3 border-b border-border bg-surface px-6 py-3">
      <h2 className="text-sm font-semibold text-text">{title}</h2>
      {hasHelp && (
        <HelpTip title={title}>
          {what && (
            <p>
              <span className="text-text-muted">What.</span> {what}
            </p>
          )}
          {how && (
            <p>
              <span className="text-text-muted">How.</span> {how}
            </p>
          )}
          {normal && (
            <p>
              <span className="text-text-muted">Healthy.</span> {normal}
            </p>
          )}
        </HelpTip>
      )}
      <span aria-hidden className="h-px flex-1 bg-border" />
      {right}
    </header>
  );
}
