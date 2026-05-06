/**
 * Page-level header. Single accent top border, a short eyebrow line, the
 * page title in IBM Plex Mono semibold, and a one-paragraph description
 * that tells the operator what they are looking at and how often it
 * refreshes. Reused at the top of every routed page so the visual
 * hierarchy stays consistent.
 */
export function PageHero({
  title,
  description,
  eyebrow = "live",
}: {
  title: string;
  description: string;
  eyebrow?: string;
}) {
  return (
    <header className="border-t-2 border-accent border-b border-border bg-surface px-6 py-7">
      <div className="flex items-baseline gap-3">
        <span aria-hidden className="font-mono text-[10px] tabular text-accent">◆</span>
        <span className="font-mono text-[10px] uppercase tracking-[0.22em] text-text-dim">
          {eyebrow}
        </span>
      </div>
      <h1 className="mt-1 font-mono text-2xl font-semibold text-text">{title}</h1>
      <p className="mt-2 max-w-3xl text-sm leading-relaxed text-text-muted">{description}</p>
    </header>
  );
}
