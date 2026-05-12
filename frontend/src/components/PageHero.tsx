import type { ReactNode } from "react";
import { HelpTip } from "./HelpTip";

/**
 * Page-level header. A single-line title with an `(i)` popover for the
 * page guidance and an optional `actions` slot for page-scoped controls
 * (e.g. "Open war room" on Overview). Earlier versions rendered the
 * description as a banner paragraph below the title; on wide monitors
 * that wasted vertical space and read like marketing chrome. The
 * popover keeps the same operator help one click away without claiming
 * a strip of the viewport.
 */
export function PageHero({
  title,
  description,
  actions,
}: {
  title: string;
  description: string;
  actions?: ReactNode;
}) {
  return (
    <header className="flex items-center gap-3 border-b border-border bg-bg px-8 pt-6 pb-4">
      <h1 className="text-2xl font-semibold tracking-tight text-text">{title}</h1>
      {description && (
        <HelpTip title={title}>
          <p>{description}</p>
        </HelpTip>
      )}
      {actions && <div className="ml-auto flex items-center gap-2">{actions}</div>}
    </header>
  );
}
