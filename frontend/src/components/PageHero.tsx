import type { ReactNode } from "react";
import { HelpTip, type HelpContent } from "./HelpTip";

/**
 * Page-level header. A single-line title with an `(i)` popover for the
 * page guidance and an optional `actions` slot for page-scoped controls
 * (e.g. "Open war room" on Overview). Help can be structured (preferred,
 * driven by the 2026 Datadog/Grafana/Vercel pattern) or free-form
 * description for legacy pages.
 */
export function PageHero({
  title,
  description,
  help,
  actions,
}: {
  title: string;
  description?: string;
  help?: HelpContent;
  actions?: ReactNode;
}) {
  return (
    <header className="flex items-center gap-3 border-b border-border bg-bg px-8 pt-6 pb-4">
      <h1 className="text-2xl font-semibold tracking-tight text-text">{title}</h1>
      {help ? (
        <HelpTip title={title} help={help} />
      ) : description ? (
        <HelpTip title={title}>
          <p>{description}</p>
        </HelpTip>
      ) : null}
      {actions && <div className="ml-auto flex items-center gap-2">{actions}</div>}
    </header>
  );
}
