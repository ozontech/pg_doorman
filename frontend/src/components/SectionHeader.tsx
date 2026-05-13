import type { ReactNode } from "react";
import { HelpTip, type HelpContent } from "./HelpTip";

interface SectionHeaderProps {
  title: string;
  /** Structured help (preferred — used for new sections). */
  help?: HelpContent;
  /** Legacy free-form fields. New code should use `help`. */
  what?: string;
  how?: string;
  normal?: string;
  /** Optional right-aligned slot (status pill, last-updated chip, button). */
  right?: ReactNode;
  /** When set, the title becomes a clickable button (used for chart cards
   * that open a full-screen PanelView on click). */
  onTitleClick?: () => void;
}

/**
 * Section header used above every chart, table, and panel. Shows the
 * title with an `(i)` icon to the right when help is provided — full
 * guidance lives in a click-popover with structured operator help:
 * one-sentence definition, admin SQL source, formula, thresholds with
 * concrete numbers, related metrics, and a docs link.
 */
export function SectionHeader({
  title,
  help,
  what,
  how,
  normal,
  right,
  onTitleClick,
}: SectionHeaderProps) {
  const hasLegacy = !help && Boolean(what || how || normal);
  return (
    <header className="flex items-center gap-3 border-b border-border bg-surface px-6 py-3">
      {onTitleClick ? (
        <button
          type="button"
          onClick={onTitleClick}
          className="text-sm font-semibold text-text hover:text-accent"
          title="Open the full-screen panel"
        >
          {title}
        </button>
      ) : (
        <h2 className="text-sm font-semibold text-text">{title}</h2>
      )}
      {help ? (
        <HelpTip title={title} help={help} />
      ) : hasLegacy ? (
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
      ) : null}
      <span aria-hidden className="h-px flex-1 bg-border" />
      {right}
    </header>
  );
}
