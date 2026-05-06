/**
 * Page-level header. A clean title above a one-paragraph description that
 * explains what the page is and how often the data refreshes. Reused at the
 * top of every routed page so the visual hierarchy stays consistent without
 * resorting to gimmicky chrome.
 */
export function PageHero({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <header className="border-b border-border bg-bg px-8 pb-7 pt-9">
      <h1 className="text-3xl font-semibold tracking-tight text-text">{title}</h1>
      <p className="mt-3 max-w-3xl text-sm leading-relaxed text-text-muted">
        {description}
      </p>
    </header>
  );
}
