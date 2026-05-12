import { useEffect, useRef, useState, type ReactNode } from "react";
import { ExternalLink, Info } from "lucide-react";

export interface HelpContent {
  /** One-sentence definition — what the metric / panel actually is. */
  definition?: string;
  /** Admin SQL the operator can run from psql (PgBouncer-compat
   * console) for the same number — e.g. `SHOW POOLS`, `SHOW STATS`.
   * Not the SPA API endpoint. */
  source?: string;
  /** Formula or computation expression, shown as monospaced code. */
  formula?: string;
  /** Thresholds with concrete numbers. */
  thresholds?: {
    healthy?: string;
    warn?: string;
    crit?: string;
  };
  /** Related metric names that appear elsewhere in the console. */
  related?: string[];
  /** Documentation anchor URL — opens in a new tab. */
  docsHref?: string;
}

interface HelpTipProps {
  title: string;
  /** Structured help — preferred. */
  help?: HelpContent;
  /** Legacy free-form children (kept for KV row labels and a few
   * call-sites that have not migrated to structured help yet). */
  children?: ReactNode;
  /** Render the icon as an inline accent on the parent text colour
   * (default) or in a circle border. */
  variant?: "icon" | "circle";
}

/**
 * Click-popover with structured operator help. Rich form follows the
 * 2026 pattern from Datadog / Grafana / Vercel: short definition,
 * source-of-truth, formula, thresholds, related metrics, link to docs.
 *
 * Trigger is a small (i) icon next to the title. Click toggles, hover
 * opens too (no commit), click outside or Escape closes. The popover
 * anchors below the trigger; max-h 60vh with overflow-y-auto so a long
 * description still stays inside the viewport.
 */
export function HelpTip({ title, help, children, variant = "icon" }: HelpTipProps) {
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

  const triggerClass =
    variant === "circle"
      ? "grid h-5 w-5 place-items-center rounded-full border border-border-strong text-[10px] font-semibold text-text-muted transition-colors hover:border-accent hover:text-accent"
      : "inline-flex items-center justify-center text-text-dim transition-colors hover:text-accent";

  return (
    <div ref={ref} className="relative inline-flex">
      <button
        type="button"
        aria-expanded={open}
        aria-label={`Help: ${title}`}
        onClick={() => setOpen((v) => !v)}
        onMouseEnter={() => setOpen(true)}
        className={triggerClass}
      >
        {variant === "circle" ? "i" : <Info size={14} strokeWidth={1.75} />}
      </button>
      {open && (
        <div className="absolute left-0 top-7 z-30">
          <div className="w-96 max-w-[calc(100vw-2rem)] max-h-[60vh] overflow-y-auto rounded-lg border border-border-strong bg-surface p-4 shadow-xl">
            <div className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-accent">
              {title}
            </div>
            {help ? <StructuredHelp help={help} /> : children ? (
              <div className="space-y-2 text-sm leading-relaxed text-text">
                {children}
              </div>
            ) : null}
          </div>
        </div>
      )}
    </div>
  );
}

function StructuredHelp({ help }: { help: HelpContent }) {
  return (
    <div className="space-y-3 text-sm leading-relaxed text-text">
      {help.definition && <p>{help.definition}</p>}
      {help.source && (
        <Row label="Source">
          <code className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-xs">
            {help.source}
          </code>
        </Row>
      )}
      {help.formula && (
        <Row label="Formula">
          <code className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-xs">
            {help.formula}
          </code>
        </Row>
      )}
      {help.thresholds && (
        <Row label="Thresholds">
          <div className="flex flex-wrap items-center gap-2 text-xs">
            {help.thresholds.healthy && (
              <span className="inline-flex items-center gap-1">
                <span className="h-2 w-2 rounded-full bg-success" />
                {help.thresholds.healthy}
              </span>
            )}
            {help.thresholds.warn && (
              <span className="inline-flex items-center gap-1">
                <span className="h-2 w-2 rounded-full bg-warning" />
                {help.thresholds.warn}
              </span>
            )}
            {help.thresholds.crit && (
              <span className="inline-flex items-center gap-1">
                <span className="h-2 w-2 rounded-full bg-danger" />
                {help.thresholds.crit}
              </span>
            )}
          </div>
        </Row>
      )}
      {help.related && help.related.length > 0 && (
        <Row label="Related">
          <div className="flex flex-wrap gap-1.5">
            {help.related.map((r) => (
              <span
                key={r}
                className="rounded-md border border-border-strong bg-surface-2 px-1.5 py-0.5 font-mono text-[11px] text-text-muted"
              >
                {r}
              </span>
            ))}
          </div>
        </Row>
      )}
      {help.docsHref && (
        <a
          href={help.docsHref}
          target="_blank"
          rel="noreferrer noopener"
          className="inline-flex items-center gap-1 text-xs font-medium text-accent hover:text-accent-hover"
        >
          Open in docs
          <ExternalLink size={12} strokeWidth={1.75} aria-hidden="true" />
        </a>
      )}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[10px] font-semibold uppercase tracking-wide text-text-dim">
        {label}
      </span>
      <div>{children}</div>
    </div>
  );
}
