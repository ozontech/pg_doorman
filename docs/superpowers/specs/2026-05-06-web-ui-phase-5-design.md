# Web UI Phase 5 — Frontend Skeleton Design

This document narrows the broader Web UI design (`2026-05-06-web-ui-design.md` and `2026-05-06-web-ui-design-system.md`) to the boundaries of phase 5: the frontend project scaffold that lands `frontend/` with a working dev loop, an authenticated shell, six placeholder pages, and a CI workflow. Real page content and bundle embedding belong to phases 6 and 7.

## 1. Goal

Land a buildable, lint-clean React + TypeScript frontend that:

- Runs under `vite dev` against a live `pg_doorman` on `http://127.0.0.1:9127` via proxy.
- Compiles and types-checks under CI without an `npm run build` step in the Rust release pipeline (decision #22 of the parent spec).
- Renders a sidebar shell and AuthGate that already handle the basic-auth flow described in §10.3 of the parent spec, so that phase 6 only needs to fill in page bodies.

The phase delivers no live data on screen; every page is a single placeholder.

## 2. Non-goals

- Page bodies (Overview, Pools, Clients, Caches, Logs, ConfigState) — phase 6.
- uPlot integration on screen — phase 6 (the dependency is added now to fix the lockfile).
- `src/web/static_assets.rs` + `include_dir!` embedding — phase 7.
- BDD scenarios for the frontend — phase 7.
- Frontend unit tests via Vitest — deferred until phase 6 (when stateful components arrive); phase 5 relies on ESLint and `tsc --noEmit` as the only quality gates.

## 3. Stack

The parent spec fixed React 18 + TypeScript 5 + Vite 5 + react-router 6 + uPlot 1.6 + Tailwind v3. Phase 5 updates two of those choices:

- **Tailwind v4** instead of v3 — the v4 CSS-first config (`@theme` directive in the entry stylesheet) removes the JavaScript config file and the PostCSS plugin, replaces them with `@tailwindcss/vite`. Less boilerplate, fewer transitive dependencies. Approved 2026-05-06 chat.
- **Vite 6** instead of Vite 5 — current LTS at the time of authoring. Drop-in for our usage; no breaking changes to the small surface we use (dev server + library config).

Versions to lock in `package.json`:

| Package | Version |
|---|---|
| react, react-dom | ^18.3 |
| react-router-dom | ^6.26 |
| typescript | ^5.5 |
| vite | ^6.0 |
| @vitejs/plugin-react | ^4.3 |
| tailwindcss | ^4.0 |
| @tailwindcss/vite | ^4.0 |
| uplot | ^1.6 |
| eslint | ^9.10 |
| @typescript-eslint/parser, @typescript-eslint/eslint-plugin | ^8.5 |
| eslint-plugin-react, eslint-plugin-react-hooks | ^5.0, ^5.0 |
| @types/react, @types/react-dom | ^18.3 |

`package-lock.json` is committed.

## 4. Layout

```
frontend/
├── package.json
├── package-lock.json
├── tsconfig.json
├── tsconfig.node.json
├── eslint.config.js
├── vite.config.ts
├── index.html
├── public/favicon.ico       (placeholder; final asset can land in phase 6)
├── src/
│   ├── main.tsx
│   ├── App.tsx              (router + layout shell)
│   ├── api.ts
│   ├── types.ts
│   ├── components/
│   │   ├── Sidebar.tsx
│   │   └── AuthGate.tsx
│   ├── hooks/
│   │   ├── usePoll.ts
│   │   └── useAdminAuth.ts
│   ├── pages/
│   │   ├── Overview.tsx
│   │   ├── Pools.tsx
│   │   ├── Clients.tsx
│   │   ├── Caches.tsx
│   │   ├── Logs.tsx
│   │   └── ConfigState.tsx
│   └── styles/
│       └── tailwind.css     (entry stylesheet with @import "tailwindcss" + @theme block)
└── dist/                    (committed)
```

`vite.config.ts` uses `defineConfig({ plugins: [react(), tailwindcss()], server: { proxy: { "/api": "http://127.0.0.1:9127" } } })`. No SSR, no library mode, no env files.

## 5. Components

### 5.1 App shell

`App.tsx` mounts `<BrowserRouter>` with six `<Route>` entries (one per page) plus a redirect from `/` to `/overview`. The layout is a two-column grid: `<Sidebar>` in the left column (fixed width ~200 px), the routed page in the right column (flex-fill). Both regions are wrapped in `<AuthGate>`, which gates whatever the page tries to fetch.

### 5.2 Sidebar

Six `<NavLink>`s pointing at the page routes. The active route gets a foreground accent (per design system tokens). No collapse behavior, no nested groups, no icons in phase 5 — those are visual polish for phase 6.

### 5.3 AuthGate

The contract codified in the parent spec §10.3:

- On any 401 response from `api.ts`, AuthGate enters a "needs-credentials" state that suspends the children and renders a modal with `username` + `password` inputs.
- On submit, credentials are stored in React state (memory-only, lost on refresh) and the failing request is retried.
- If the retry succeeds, the modal closes; if it returns 401 again, the modal stays open with an inline error message.

The hook `useAdminAuth` owns the credentials state and the `Authorization` header builder. `api.ts` reads the current credentials at call time, so a credential update propagates without component re-mount gymnastics.

In phase 5 the only requests are an initial `GET /api/version` mount-time check (so we know whether AuthGate triggers immediately for `ui_anonymous=false` deployments) and the AuthGate-driven retry. Phase 6 pages add their own polled requests.

## 6. Hooks

- `usePoll(fetcher, intervalMs)` — `useEffect` wrapper that calls `fetcher` on mount and every `intervalMs`, exposes `{ data, error, lastUpdated }`. Cancels via `AbortController` on unmount and on dependency change. Default interval is the parent spec's 1500 ms; phase 5 calls it only from AuthGate's version probe (longer interval is fine but the API is consistent).
- `useAdminAuth()` — returns `{ creds, setCreds, header }` where `header` is `() => Record<string, string>` returning either `{ Authorization: "Basic …" }` or `{}` if no creds.

`useUrlState`, `useHistory`, `useThresholdPaint`, `useKeyboard` are deferred to phase 6.

## 7. API client (`api.ts` + `types.ts`)

`api.ts` is a single typed `fetch` wrapper:

```ts
export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { ...init, headers: { ...init?.headers, ...authHeader() } });
  if (res.status === 401) throw new Unauthorized();
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return res.json();
}
```

`Unauthorized` is a sentinel error that AuthGate catches.

`types.ts` mirrors only the DTOs that phase 5 actually touches: `VersionDto`. The full type set (matching `src/web/routes/dto.rs`) lands incrementally in phase 6 alongside each page that needs it. Hand-maintained; no codegen — the surface is small enough that a regenerator would be more brittle than the typing it replaces.

## 8. Build & dist

- `npm run build` produces `frontend/dist/` (gzipped 150–250 KB target per parent spec).
- Phase 5 commits the initial built skeleton bundle. Future PRs that touch `frontend/src/` must rebuild and commit `dist` in the same change; CI enforces this with `git diff --exit-code frontend/dist`.
- The Rust release pipeline does **not** run `npm run build`; it embeds the committed `dist` via `include_dir!` in phase 7.

## 9. CI workflow

New file: `.github/workflows/frontend.yml`.

Trigger:
- `pull_request` paths: `frontend/**`, `.github/workflows/frontend.yml`.
- `push` on `master` for the same paths.

Steps (single Ubuntu runner, Node 20):

1. `actions/checkout@v4`.
2. `actions/setup-node@v4` with `cache: npm`, `cache-dependency-path: frontend/package-lock.json`.
3. `npm ci --prefix frontend`.
4. `npm run --prefix frontend lint` (ESLint, fails on errors).
5. `npm run --prefix frontend typecheck` (`tsc --noEmit`).
6. `npm run --prefix frontend build`.
7. `git diff --exit-code frontend/dist` — fails if the rebuilt bundle differs from the committed one.

The workflow is independent from the Rust release jobs and adds no dependency to RPM/DEB/Docker builds.

## 10. Design system mapping

`2026-05-06-web-ui-design-system.md` is the source of truth for tokens. Phase 5 transcribes its `:root` block into a Tailwind v4 `@theme` block in `frontend/src/styles/tailwind.css`. Tailwind v4 generates utilities from `@theme` variables prefixed with `--color-*`, `--font-*`, `--font-size-*` etc., so we rename the design-system tokens to those namespaces while preserving the values exactly:

```css
/* frontend/src/styles/tailwind.css */
@import "tailwindcss";

@theme {
  /* Surface — values from design-system.md §4.1 */
  --color-bg:         #0a0d12;
  --color-surface:    #11151c;
  --color-surface-2:  #161b24;
  --color-surface-3:  #1c2230;

  /* Border */
  --color-border:        #232a36;
  --color-border-strong: #2d3543;

  /* Text */
  --color-text:       #e6e9ee;
  --color-text-muted: #8a93a4;
  --color-text-dim:   #5a6275;

  /* Accent */
  --color-accent:       #22b8cf;
  --color-accent-hover: #3ec8d9;
  --color-accent-fg:    #042024;

  /* Semantic */
  --color-success: #2dc26b;
  --color-warning: #f5a524;
  --color-danger:  #e5484d;
  --color-info:    #5b8cff;

  /* Chart palette */
  --color-chart-1: #22b8cf;
  --color-chart-2: #2dc26b;
  --color-chart-3: #f5a524;
  --color-chart-4: #b18cf5;

  /* Typography — values from design-system.md §3 */
  --font-sans: "IBM Plex Sans", system-ui, sans-serif;
  --font-mono: "IBM Plex Mono", ui-monospace, monospace;
  --font-size-xs:   11px;
  --font-size-sm:   13px;
  --font-size-base: 14px;
  --font-size-md:   16px;
  --font-size-lg:   20px;
  --font-size-xl:   28px;
}
```

Phase 6 components reference `bg-surface`, `text-text-muted`, `border-border`, `text-accent` etc. instead of hex literals. If design-system.md changes, the `@theme` block is the single mirror that needs updating.

**Fonts:** IBM Plex Sans/Mono are released under the SIL Open Font License 1.1, which permits redistribution including bundling into web apps. Phase 5 self-hosts the WOFF2 files under `frontend/public/fonts/` and references them via `@font-face` in `tailwind.css`. (If WOFF2 fetching fails to land cleanly during implementation, the stylesheet falls back to system fonts and phase 6 picks up self-hosted fonts — but the licensing check is already done, so this is implementation-mechanical, not a design risk.)

## 11. Out-of-scope (phase 6 / phase 7 deferrals)

- Page implementations, uPlot integration, `src/lib/thresholds.ts`, `useHistory`, `useUrlState`.
- Vitest test setup.
- `LogStream`, `Heatmap`, `Sparkline`, `Drawer`, `TimePicker`, `EmptyState`, `Banner`, `Badge`, `Button` components.
- `src/web/static_assets.rs` and the `include_dir!` build wiring.
- BDD `.feature` files referencing the UI.
- Pre-commit hook for auto-rebuild — left as a developer convenience (parent spec §10.4 already labels it nice-to-have); CI catches stale `dist` regardless.

## 12. Implementation phases (this spec → plan)

The implementation plan (`writing-plans` next step) will break phase 5 into roughly:

1. Scaffold `frontend/` with `npm init`-equivalent + dependencies + tsconfig + Vite config + ESLint config.
2. Tailwind v4 entry stylesheet with `@theme` tokens + IBM Plex font hosting.
3. App shell: `main.tsx`, `App.tsx`, `Sidebar`, six placeholder pages, react-router wiring.
4. AuthGate + `useAdminAuth` + `api.ts` + `types.ts` (VersionDto).
5. `usePoll` hook.
6. Initial `npm run build` → commit `frontend/dist/`.
7. `.github/workflows/frontend.yml` + verification.
8. Smoke: `vite dev` against a live `pg_doorman`, navigate sidebar, trigger AuthGate by hitting an admin-anonymous-disabled config.

Each step is a single subagent task; the phase commits as a single `feat(web): land frontend skeleton (phase 5)` once all gates pass.

## 13. Risks and migration notes

- **Lockfile churn:** the initial `npm install` will produce a long `package-lock.json`. We accept this; subsequent dependency upgrades land via dedicated PRs.
- **Tailwind v4 maturity:** v4 is stable as of late 2024 / early 2025 but ecosystem coverage of plugins is thinner than v3. We do not use any third-party Tailwind plugin in phase 5; the `@theme` block is self-contained. If a phase 6 component needs e.g. `@tailwindcss/typography`, we re-evaluate.
- **`dist` size in repo:** parent spec budgets 150–250 KB gzipped. The skeleton is well under that (no real components yet). Watch the size at each phase 6 page commit.
- **Font licensing:** verify SIL OFL on IBM Plex during implementation; fall back to system fonts if there is any doubt about hosting redistribution. This is a one-line decision in `tailwind.css`.
