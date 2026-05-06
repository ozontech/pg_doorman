# Web UI Phase 5 — Frontend Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land a `frontend/` project on `feat/web-ui` with Vite + React 18 + TypeScript 5 + Tailwind v4 + react-router 6, working `vite dev` against a live `pg_doorman`, six page placeholders, a functional AuthGate + Sidebar shell, the typed API client surface, a built bundle committed to `frontend/dist/`, and a `frontend.yml` CI workflow that lints, typechecks, and verifies the bundle is in sync — all delivered as a single phase commit (no Rust changes).

**Architecture:** New top-level `frontend/` with conventional Vite project layout. App shell is a two-column grid (Sidebar + routed page) wrapped in AuthGate. AuthGate suspends children when any `api.ts` call returns 401 and renders a basic-auth modal whose credentials live in React state (memory-only) for the session. `usePoll` and `useAdminAuth` hooks are landed empty-of-callers so phase 6 has the primitives ready.

**Tech Stack:** Node 20 (CI), Vite 6, React 18, TypeScript 5, react-router 6, Tailwind v4 (`@tailwindcss/vite`), uPlot 1.6 (dep only, used in phase 6), ESLint 9 flat config, `@fontsource/ibm-plex-sans` + `@fontsource/ibm-plex-mono` for SIL OFL font hosting.

**Reference:**
- Spec: `docs/superpowers/specs/2026-05-06-web-ui-phase-5-design.md` (phase 5 boundaries).
- Parent spec: `docs/superpowers/specs/2026-05-06-web-ui-design.md` §4.3, §10.
- Design system: `docs/superpowers/specs/2026-05-06-web-ui-design-system.md` §3, §4.

**Out of scope (deferred to phase 6 or 7):**
- Page bodies, uPlot integration, threshold logic, `useHistory`, `useUrlState`.
- Rust-side `src/web/static_assets.rs` + `include_dir!` (phase 7).
- BDD scenarios (phase 7).
- Vitest (phase 6 when stateful components arrive).

**Commit policy:** All work goes into a **single phase commit** at the very end (Task 13), matching the phase 4 pattern. Tasks 1–12 do not commit. Built `frontend/dist/` is committed as part of the phase commit.

---

## Task 0: Baseline

```bash
cd /home/vadv/Projects/pg_doorman
git status                           # expect clean tree on feat/web-ui (untracked .local/, Dockerfile.ubuntu22-tls, INCIDENT_*.md are pre-existing — ignore)
git log --oneline -3                 # expect HEAD = b98ef81 (phase 5 design spec)
test ! -d frontend && echo "ok no frontend yet" || echo "FAIL: frontend/ already exists"
which node                           # node 20+ required
node --version                       # expect v20.x or v22.x
which npm
npm --version                        # expect 10.x+
```

If `node`/`npm` are missing, install Node 20 (`nvm install 20 && nvm use 20`).

If `frontend/` already exists, abort and ask the user.

---

## Task 1: Initialize `frontend/` and dependencies

**Files:**
- Create: `frontend/package.json`
- Create: `frontend/.gitignore`

- [ ] **Step 1.1: Create frontend directory**

```bash
cd /home/vadv/Projects/pg_doorman
mkdir frontend
cd frontend
```

- [ ] **Step 1.2: Initialise npm project**

```bash
npm init -y
```

This creates a default `package.json`. Replace it with the canonical content below — `npm init -y` populates fields we don't want.

- [ ] **Step 1.3: Write `package.json`**

Replace `frontend/package.json` with:

```json
{
  "name": "pg-doorman-web",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "preview": "vite preview",
    "lint": "eslint .",
    "typecheck": "tsc --noEmit"
  },
  "dependencies": {
    "@fontsource/ibm-plex-mono": "^5.1.1",
    "@fontsource/ibm-plex-sans": "^5.1.1",
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "react-router-dom": "^6.27.0",
    "uplot": "^1.6.31"
  },
  "devDependencies": {
    "@tailwindcss/vite": "^4.0.0",
    "@types/react": "^18.3.12",
    "@types/react-dom": "^18.3.1",
    "@typescript-eslint/eslint-plugin": "^8.13.0",
    "@typescript-eslint/parser": "^8.13.0",
    "@vitejs/plugin-react": "^4.3.3",
    "eslint": "^9.14.0",
    "eslint-plugin-react": "^7.37.2",
    "eslint-plugin-react-hooks": "^5.0.0",
    "tailwindcss": "^4.0.0",
    "typescript": "^5.6.3",
    "vite": "^6.0.0"
  }
}
```

(Lockfile is regenerated in step 1.5; the version pins above are floors — `npm install` will resolve exact versions.)

- [ ] **Step 1.4: Write `.gitignore`**

Create `frontend/.gitignore`:

```
node_modules/
.vite/
*.local
.eslintcache
```

`dist/` is **not** ignored — we commit the built bundle (decision #22 of the parent spec).

- [ ] **Step 1.5: Install**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm install
```

Expected: a `package-lock.json` appears, `node_modules/` populates. If npm reports peer-dep warnings only, that's OK; outright errors are not.

- [ ] **Step 1.6: Sanity**

```bash
ls package-lock.json node_modules >/dev/null && echo ok
```

Expected: `ok`.

- [ ] **Step 1.7: DO NOT commit.**

---

## Task 2: TypeScript and ESLint configuration

**Files:**
- Create: `frontend/tsconfig.json`
- Create: `frontend/tsconfig.node.json`
- Create: `frontend/eslint.config.js`

- [ ] **Step 2.1: Write `tsconfig.json`**

`frontend/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "useDefineForClassFields": true,
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,

    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "moduleDetection": "force",
    "noEmit": true,
    "jsx": "react-jsx",

    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "noUncheckedSideEffectImports": true
  },
  "include": ["src"],
  "references": [{ "path": "./tsconfig.node.json" }]
}
```

- [ ] **Step 2.2: Write `tsconfig.node.json`**

`frontend/tsconfig.node.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2023"],
    "module": "ESNext",
    "skipLibCheck": true,

    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "moduleDetection": "force",
    "noEmit": true,

    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true
  },
  "include": ["vite.config.ts"]
}
```

- [ ] **Step 2.3: Write `eslint.config.js`**

`frontend/eslint.config.js` (flat config, ESLint 9):

```js
import js from "@eslint/js";
import tsParser from "@typescript-eslint/parser";
import tsPlugin from "@typescript-eslint/eslint-plugin";
import reactPlugin from "eslint-plugin-react";
import reactHooksPlugin from "eslint-plugin-react-hooks";
import globals from "globals";

export default [
  { ignores: ["dist", "node_modules"] },
  js.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      parser: tsParser,
      parserOptions: { ecmaVersion: "latest", sourceType: "module", ecmaFeatures: { jsx: true } },
      globals: { ...globals.browser },
    },
    plugins: {
      "@typescript-eslint": tsPlugin,
      "react": reactPlugin,
      "react-hooks": reactHooksPlugin,
    },
    settings: { react: { version: "18.3" } },
    rules: {
      ...tsPlugin.configs.recommended.rules,
      ...reactPlugin.configs.recommended.rules,
      ...reactHooksPlugin.configs.recommended.rules,
      "react/react-in-jsx-scope": "off",
      "react/prop-types": "off",
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_" }],
    },
  },
];
```

- [ ] **Step 2.4: Add `globals` package**

`eslint.config.js` imports `globals`. Install:

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm install --save-dev globals
```

Also install `@eslint/js`:

```bash
npm install --save-dev @eslint/js
```

- [ ] **Step 2.5: Verify gates (configs only — no source yet, so lint/typecheck pass trivially)**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run lint 2>&1 | tail -5
npm run typecheck 2>&1 | tail -5
```

Expected: both exit 0 (no source files to check yet — the `include: ["src"]` in tsconfig will be empty, ESLint matches zero files; both succeed).

- [ ] **Step 2.6: DO NOT commit.**

---

## Task 3: Vite config, index.html, app entrypoint

**Files:**
- Create: `frontend/vite.config.ts`
- Create: `frontend/index.html`
- Create: `frontend/src/main.tsx`
- Create: `frontend/src/App.tsx` (placeholder body, full version lands in Task 8)

- [ ] **Step 3.1: Write `vite.config.ts`**

`frontend/vite.config.ts`:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/api": "http://127.0.0.1:9127",
      "/metrics": "http://127.0.0.1:9127",
    },
  },
  build: {
    outDir: "dist",
    sourcemap: false,
    chunkSizeWarningLimit: 500,
  },
});
```

`/metrics` is proxied so a curious operator running `vite dev` can also hit the Prometheus endpoint without a separate port.

- [ ] **Step 3.2: Write `index.html`**

`frontend/index.html`:

```html
<!doctype html>
<html lang="en" class="bg-bg text-text">
  <head>
    <meta charset="UTF-8" />
    <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>pg_doorman</title>
  </head>
  <body class="font-sans">
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

The `bg-bg`/`text-text`/`font-sans` classes resolve to design-system tokens once Task 4 lands the `@theme` block.

- [ ] **Step 3.3: Write a placeholder `favicon.svg`**

`frontend/public/favicon.svg`:

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect width="16" height="16" fill="#22b8cf"/>
  <text x="50%" y="58%" text-anchor="middle" font-family="monospace" font-size="11" fill="#0a0d12">pd</text>
</svg>
```

(Phase 6 may replace with a polished asset.)

- [ ] **Step 3.4: Write `src/main.tsx`**

`frontend/src/main.tsx`:

```tsx
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles/tailwind.css";

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("missing #root in index.html");
}
createRoot(rootEl).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
```

- [ ] **Step 3.5: Write a temporary `src/App.tsx`**

`frontend/src/App.tsx`:

```tsx
export default function App() {
  return <div className="p-4">pg_doorman web shell — phase 5 scaffold</div>;
}
```

This is replaced in Task 8 with the real router + sidebar shell. Keeping it as a placeholder for now lets us verify the build pipeline before we touch routing.

- [ ] **Step 3.6: Build sanity (will fail because tailwind.css doesn't exist yet)**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run build 2>&1 | tail -10
```

Expected: build fails with `Could not resolve "./styles/tailwind.css"`. That's intentional — Task 4 lands the stylesheet. Move on.

- [ ] **Step 3.7: DO NOT commit.**

---

## Task 4: Tailwind v4 entry stylesheet with design-system tokens

**Files:**
- Create: `frontend/src/styles/tailwind.css`

- [ ] **Step 4.1: Write `tailwind.css`**

`frontend/src/styles/tailwind.css`:

```css
@import "tailwindcss";

/* Self-hosted IBM Plex via @fontsource — both packages register @font-face declarations on import. */
@import "@fontsource/ibm-plex-sans/400.css";
@import "@fontsource/ibm-plex-sans/500.css";
@import "@fontsource/ibm-plex-sans/600.css";
@import "@fontsource/ibm-plex-mono/400.css";
@import "@fontsource/ibm-plex-mono/500.css";

@theme {
  /* Surface — values from design-system.md §4.1 (verbatim) */
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

  /* Chart palette (uPlot lines) */
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

/* Tabular figures helper (design-system.md §3.4). */
.tabular {
  font-variant-numeric: tabular-nums slashed-zero;
}
```

- [ ] **Step 4.2: Build sanity**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run build 2>&1 | tail -10
```

Expected: build succeeds. Output mentions `dist/index.html`, `dist/assets/index-*.css`, `dist/assets/index-*.js`. CSS bundle is small (no real components yet).

- [ ] **Step 4.3: Lint + typecheck**

```bash
npm run lint 2>&1 | tail -5
npm run typecheck 2>&1 | tail -5
```

Expected: both clean.

- [ ] **Step 4.4: DO NOT commit.**

---

## Task 5: Auth primitives — types, useAdminAuth, api.ts

**Files:**
- Create: `frontend/src/types.ts`
- Create: `frontend/src/hooks/useAdminAuth.ts`
- Create: `frontend/src/api.ts`

- [ ] **Step 5.1: Write `types.ts`**

`frontend/src/types.ts`:

```ts
/**
 * DTO mirrors. Phase 5 only exposes the types phase 5 actually uses; phase 6
 * adds the rest as pages need them. Source of truth is `src/web/routes/dto.rs`
 * — keep these manual until divergence becomes painful.
 */
export interface VersionDto {
  version: string;
  git_commit: string;
  build_date: string;
  ts: number;
}
```

- [ ] **Step 5.2: Write `useAdminAuth.ts`**

`frontend/src/hooks/useAdminAuth.ts`:

```ts
import { createContext, useCallback, useContext, useState, type ReactNode } from "react";

interface Credentials {
  username: string;
  password: string;
}

interface AdminAuthValue {
  creds: Credentials | null;
  setCreds: (next: Credentials | null) => void;
  authHeader: () => Record<string, string>;
}

const AdminAuthContext = createContext<AdminAuthValue | null>(null);

export function AdminAuthProvider({ children }: { children: ReactNode }) {
  const [creds, setCreds] = useState<Credentials | null>(null);

  const authHeader = useCallback((): Record<string, string> => {
    if (!creds) return {};
    const token = btoa(`${creds.username}:${creds.password}`);
    return { Authorization: `Basic ${token}` };
  }, [creds]);

  return (
    <AdminAuthContext.Provider value={{ creds, setCreds, authHeader }}>
      {children}
    </AdminAuthContext.Provider>
  );
}

export function useAdminAuth(): AdminAuthValue {
  const ctx = useContext(AdminAuthContext);
  if (!ctx) throw new Error("useAdminAuth must be used inside AdminAuthProvider");
  return ctx;
}
```

(File extension: `.tsx` because the provider returns JSX. Rename if you prefer; the import path matters more than the extension.)

**Important:** rename to `useAdminAuth.tsx` — the file contains JSX.

```bash
cd /home/vadv/Projects/pg_doorman/frontend/src/hooks
mv useAdminAuth.ts useAdminAuth.tsx 2>/dev/null || true
```

- [ ] **Step 5.3: Write `api.ts`**

`frontend/src/api.ts`:

```ts
/**
 * Typed fetch wrapper. Reads credentials from the AdminAuth context lazily
 * via the headerProvider param, so a credential update in AuthGate
 * propagates to in-flight retries without component remounting.
 */
export class Unauthorized extends Error {
  constructor() {
    super("401 Unauthorized");
    this.name = "Unauthorized";
  }
}

export class ApiError extends Error {
  constructor(public readonly status: number, public readonly body: string) {
    super(`api ${status}: ${body.slice(0, 200)}`);
    this.name = "ApiError";
  }
}

export type HeaderProvider = () => Record<string, string>;

export async function apiGet<T>(
  path: string,
  headerProvider: HeaderProvider,
  signal?: AbortSignal,
): Promise<T> {
  const res = await fetch(path, {
    method: "GET",
    headers: { Accept: "application/json", ...headerProvider() },
    signal,
  });
  if (res.status === 401) throw new Unauthorized();
  if (!res.ok) throw new ApiError(res.status, await res.text());
  return (await res.json()) as T;
}
```

- [ ] **Step 5.4: Lint + typecheck**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run lint 2>&1 | tail -5
npm run typecheck 2>&1 | tail -5
```

Expected: both clean.

- [ ] **Step 5.5: DO NOT commit.**

---

## Task 6: AuthGate component

**Files:**
- Create: `frontend/src/components/AuthGate.tsx`

- [ ] **Step 6.1: Write `AuthGate.tsx`**

`frontend/src/components/AuthGate.tsx`:

```tsx
import { useEffect, useState, type ReactNode } from "react";
import { apiGet, Unauthorized } from "../api";
import { useAdminAuth } from "../hooks/useAdminAuth";
import type { VersionDto } from "../types";

/**
 * Probes /api/version on mount and on credential change. If the probe returns
 * 401, renders a basic-auth modal that locks the rest of the app until the
 * user submits credentials that satisfy /api/version. Once authorised (or
 * if /api/version is anonymously accessible), renders children.
 *
 * Phase 5 only owns the version probe. Phase 6 pages drive the same auth
 * flow indirectly: every apiGet call rethrows Unauthorized; the gate shows
 * the modal until a successful retry.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const { creds, setCreds, authHeader } = useAdminAuth();
  const [needsAuth, setNeedsAuth] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [probing, setProbing] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setProbing(true);
    setError(null);
    apiGet<VersionDto>("/api/version", authHeader)
      .then(() => {
        if (cancelled) return;
        setNeedsAuth(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        if (e instanceof Unauthorized) {
          setNeedsAuth(true);
        } else {
          setError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!cancelled) setProbing(false);
      });
    return () => {
      cancelled = true;
    };
  }, [authHeader]);

  if (probing) {
    return <div className="p-4 text-text-muted">connecting…</div>;
  }
  if (error) {
    return <div className="p-4 text-danger">{error}</div>;
  }
  if (needsAuth) {
    return <AuthModal currentCreds={creds} onSubmit={setCreds} />;
  }
  return <>{children}</>;
}

function AuthModal({
  currentCreds,
  onSubmit,
}: {
  currentCreds: { username: string; password: string } | null;
  onSubmit: (next: { username: string; password: string }) => void;
}) {
  const [username, setUsername] = useState(currentCreds?.username ?? "");
  const [password, setPassword] = useState("");
  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    onSubmit({ username, password });
  };
  return (
    <div className="fixed inset-0 flex items-center justify-center bg-bg/80 backdrop-blur-sm">
      <form
        onSubmit={submit}
        className="w-80 rounded border border-border bg-surface p-6 shadow-xl"
      >
        <h2 className="mb-4 text-md font-semibold">Sign in</h2>
        <p className="mb-4 text-sm text-text-muted">
          {currentCreds
            ? "Credentials were rejected. Try again."
            : "This pg_doorman requires admin credentials."}
        </p>
        <label className="mb-2 block text-xs uppercase tracking-wide text-text-muted">
          Username
        </label>
        <input
          autoFocus
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          className="mb-3 w-full rounded border border-border-strong bg-surface-2 px-2 py-1.5 text-sm text-text"
        />
        <label className="mb-2 block text-xs uppercase tracking-wide text-text-muted">
          Password
        </label>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          className="mb-4 w-full rounded border border-border-strong bg-surface-2 px-2 py-1.5 text-sm text-text"
        />
        <button
          type="submit"
          className="w-full rounded bg-accent px-3 py-1.5 text-sm font-medium text-accent-fg hover:bg-accent-hover"
        >
          Sign in
        </button>
      </form>
    </div>
  );
}
```

- [ ] **Step 6.2: Verify gates**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run lint 2>&1 | tail -5
npm run typecheck 2>&1 | tail -5
```

Expected: both clean.

- [ ] **Step 6.3: DO NOT commit.**

---

## Task 7: usePoll hook

**Files:**
- Create: `frontend/src/hooks/usePoll.ts`

- [ ] **Step 7.1: Write `usePoll.ts`**

`frontend/src/hooks/usePoll.ts`:

```ts
import { useEffect, useRef, useState } from "react";

interface PollState<T> {
  data: T | null;
  error: Error | null;
  lastUpdated: number | null;
}

/**
 * Calls fetcher on mount and every intervalMs ms. Cancels the in-flight
 * request via AbortController on unmount and on dependency change. Phase 5
 * does not call this hook from any page; it is here so phase 6 has the
 * primitive ready.
 */
export function usePoll<T>(
  fetcher: (signal: AbortSignal) => Promise<T>,
  intervalMs = 1500,
): PollState<T> {
  const [state, setState] = useState<PollState<T>>({
    data: null,
    error: null,
    lastUpdated: null,
  });
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;

  useEffect(() => {
    let cancelled = false;
    const controller = new AbortController();
    const tick = () => {
      fetcherRef
        .current(controller.signal)
        .then((data) => {
          if (cancelled) return;
          setState({ data, error: null, lastUpdated: Date.now() });
        })
        .catch((e: unknown) => {
          if (cancelled) return;
          if (e instanceof DOMException && e.name === "AbortError") return;
          setState((prev) => ({
            ...prev,
            error: e instanceof Error ? e : new Error(String(e)),
          }));
        });
    };
    tick();
    const id = window.setInterval(tick, intervalMs);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearInterval(id);
    };
  }, [intervalMs]);

  return state;
}
```

- [ ] **Step 7.2: Verify gates**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run lint 2>&1 | tail -5
npm run typecheck 2>&1 | tail -5
```

Expected: both clean. ESLint may flag `react-hooks/exhaustive-deps` for the `fetcherRef` pattern; if it does, add the suppressing eslint-disable comment with a justifying line — but it usually doesn't because we read `.current` not the function itself.

- [ ] **Step 7.3: DO NOT commit.**

---

## Task 8: Sidebar + page placeholders + router shell

**Files:**
- Create: `frontend/src/components/Sidebar.tsx`
- Create: `frontend/src/pages/Overview.tsx`
- Create: `frontend/src/pages/Pools.tsx`
- Create: `frontend/src/pages/Clients.tsx`
- Create: `frontend/src/pages/Caches.tsx`
- Create: `frontend/src/pages/Logs.tsx`
- Create: `frontend/src/pages/ConfigState.tsx`
- Modify: `frontend/src/App.tsx` (replace Task 3 placeholder with the real shell)

- [ ] **Step 8.1: Write `Sidebar.tsx`**

`frontend/src/components/Sidebar.tsx`:

```tsx
import { NavLink } from "react-router-dom";

const NAV = [
  { to: "/overview", label: "Overview" },
  { to: "/pools", label: "Pools" },
  { to: "/clients", label: "Clients" },
  { to: "/caches", label: "Caches" },
  { to: "/logs", label: "Logs" },
  { to: "/config", label: "Config" },
];

export function Sidebar() {
  return (
    <nav className="flex h-screen w-48 flex-col border-r border-border bg-surface">
      <div className="px-4 py-4 text-md font-semibold text-text">pg_doorman</div>
      <ul className="flex-1">
        {NAV.map((item) => (
          <li key={item.to}>
            <NavLink
              to={item.to}
              className={({ isActive }) =>
                `block px-4 py-2 text-sm ${
                  isActive
                    ? "bg-surface-2 text-accent border-l-2 border-accent"
                    : "text-text-muted hover:bg-surface-2 hover:text-text"
                }`
              }
            >
              {item.label}
            </NavLink>
          </li>
        ))}
      </ul>
      <div className="px-4 py-3 text-xs text-text-dim border-t border-border">
        phase 5 skeleton
      </div>
    </nav>
  );
}
```

- [ ] **Step 8.2: Write the six page placeholders**

`frontend/src/pages/Overview.tsx`:

```tsx
export default function Overview() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Overview</h1>
      <p className="mt-2 text-sm text-text-muted">
        Placeholder page; phase 6 wires up Health bar, Golden Signals, and 3 min sparklines.
      </p>
    </section>
  );
}
```

`frontend/src/pages/Pools.tsx`:

```tsx
export default function Pools() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Pools</h1>
      <p className="mt-2 text-sm text-text-muted">
        Placeholder page; phase 6 wires up the pools table and per-pool drawer.
      </p>
    </section>
  );
}
```

`frontend/src/pages/Clients.tsx`:

```tsx
export default function Clients() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Clients</h1>
      <p className="mt-2 text-sm text-text-muted">
        Placeholder page; phase 6 wires up the clients table with filter, sort, and url-state.
      </p>
    </section>
  );
}
```

`frontend/src/pages/Caches.tsx`:

```tsx
export default function Caches() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Caches</h1>
      <p className="mt-2 text-sm text-text-muted">
        Placeholder page; phase 6 splits Prepared and Query Cache tabs.
      </p>
    </section>
  );
}
```

`frontend/src/pages/Logs.tsx`:

```tsx
export default function Logs() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Logs</h1>
      <p className="mt-2 text-sm text-text-muted">
        Placeholder page; phase 6 wires up LogStream against /api/logs.
      </p>
    </section>
  );
}
```

`frontend/src/pages/ConfigState.tsx`:

```tsx
export default function ConfigState() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Config</h1>
      <p className="mt-2 text-sm text-text-muted">
        Placeholder page; phase 6 wires up config + auth_query + log_level + databases + users + sockets + pool scaling/coordinator.
      </p>
    </section>
  );
}
```

- [ ] **Step 8.3: Replace `App.tsx` with the real shell**

`frontend/src/App.tsx`:

```tsx
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { AuthGate } from "./components/AuthGate";
import { Sidebar } from "./components/Sidebar";
import { AdminAuthProvider } from "./hooks/useAdminAuth";
import Overview from "./pages/Overview";
import Pools from "./pages/Pools";
import Clients from "./pages/Clients";
import Caches from "./pages/Caches";
import Logs from "./pages/Logs";
import ConfigState from "./pages/ConfigState";

export default function App() {
  return (
    <AdminAuthProvider>
      <BrowserRouter>
        <AuthGate>
          <div className="flex min-h-screen bg-bg text-text">
            <Sidebar />
            <main className="flex-1">
              <Routes>
                <Route path="/" element={<Navigate to="/overview" replace />} />
                <Route path="/overview" element={<Overview />} />
                <Route path="/pools" element={<Pools />} />
                <Route path="/clients" element={<Clients />} />
                <Route path="/caches" element={<Caches />} />
                <Route path="/logs" element={<Logs />} />
                <Route path="/config" element={<ConfigState />} />
                <Route path="*" element={<NotFound />} />
              </Routes>
            </main>
          </div>
        </AuthGate>
      </BrowserRouter>
    </AdminAuthProvider>
  );
}

function NotFound() {
  return (
    <section className="p-6">
      <h1 className="text-lg font-semibold text-text">Not found</h1>
    </section>
  );
}
```

- [ ] **Step 8.4: Verify gates**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run lint 2>&1 | tail -5
npm run typecheck 2>&1 | tail -5
npm run build 2>&1 | tail -10
```

Expected: all clean. Build emits `dist/index.html`, `dist/assets/index-*.{css,js}`. Bundle size should be well under 500 KB raw, well under 250 KB gzipped (still no real components).

- [ ] **Step 8.5: DO NOT commit.**

---

## Task 9: Initial committed `dist/` snapshot

**Files:**
- Create: `frontend/dist/*` (build output, committed)

- [ ] **Step 9.1: Clean rebuild**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
rm -rf dist
npm run build 2>&1 | tail -10
```

Expected: build succeeds. `dist/index.html` references the hashed asset filenames.

- [ ] **Step 9.2: Inspect dist size**

```bash
du -sh dist
ls dist/assets
```

Note the gzipped size from the Vite build summary. Expected: under 200 KB gzipped JS + ~5–20 KB CSS.

- [ ] **Step 9.3: DO NOT commit.** (Commit happens in Task 13.)

---

## Task 10: GitHub Actions workflow for frontend lint/typecheck/dist-sync

**Files:**
- Create: `.github/workflows/frontend.yml`

- [ ] **Step 10.1: Write the workflow**

`.github/workflows/frontend.yml`:

```yaml
name: Frontend lint, typecheck, dist sync

on:
  push:
    branches: [master]
    paths:
      - "frontend/**"
      - ".github/workflows/frontend.yml"
  pull_request:
    paths:
      - "frontend/**"
      - ".github/workflows/frontend.yml"

jobs:
  lint-typecheck-build:
    name: lint + typecheck + dist sync
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: npm
          cache-dependency-path: frontend/package-lock.json

      - name: Install
        run: npm ci
        working-directory: frontend

      - name: Lint
        run: npm run lint
        working-directory: frontend

      - name: Typecheck
        run: npm run typecheck
        working-directory: frontend

      - name: Build
        run: npm run build
        working-directory: frontend

      - name: Verify committed dist matches rebuild
        run: |
          if ! git diff --exit-code frontend/dist; then
            echo "::error::frontend/dist is out of sync with frontend/src."
            echo "Run \`npm run build\` in frontend/ locally and commit the result."
            exit 1
          fi
```

- [ ] **Step 10.2: Lint the YAML by feel (no `act` available)**

Visually inspect: 4-space indentation consistent, every `with:` has its keys, paths arrays use leading dashes. No further validation in this task — CI itself will tell us if the file is malformed.

- [ ] **Step 10.3: DO NOT commit.**

---

## Task 11: Smoke test against a live `pg_doorman`

**Files:**
- (none modified)

This task is a manual smoke check; no agent should automate it without the user agreeing. If running in a subagent context, report DONE_WITH_CONCERNS noting the smoke is pending a human run, and the controller can decide.

- [ ] **Step 11.1: Build and run pg_doorman with phase 4 config**

In one terminal:

```bash
cd /home/vadv/Projects/pg_doorman
./target/release/pg_doorman /tmp/doorman-phase3a.toml
```

(If the binary is stale relative to phase 4, rebuild: `cargo build --release`.)

Expected: pg_doorman bound on `127.0.0.1:19127`. The phase 4 commit set the web listener up; phase 5 doesn't change Rust code.

- [ ] **Step 11.2: Vite dev in another terminal**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run dev
```

Expected: Vite reports `Local: http://localhost:5173/`.

**Note on port mismatch:** the phase 3a config binds the web listener on port `19127`, but `vite.config.ts` proxies `/api` to `127.0.0.1:9127`. Adjust one of:
- Edit `frontend/vite.config.ts` to point at `:19127` (temporary, do not commit), OR
- Edit `/tmp/doorman-phase3a.toml` to set `[web].port = 9127` and restart pg_doorman.

The committed `vite.config.ts` targets the spec'd default port `9127`. The phase 3a config used `19127` as a non-conflict-with-existing default.

- [ ] **Step 11.3: Browser checks**

Open `http://localhost:5173/`:

1. Sidebar shows six nav items, "Overview" active by default after the redirect from `/`.
2. Each nav item navigates and shows the corresponding placeholder body.
3. AuthGate behavior: if pg_doorman is configured with `web.ui_anonymous = false`, expect the modal on first load. Submit `admin` / the configured password — modal closes, page renders.
4. Open browser devtools network tab: confirm a `GET /api/version` is made; with valid creds it returns 200.

If anything fails, fix in the relevant earlier task (do not patch live in Task 11).

- [ ] **Step 11.4: Stop both processes**

```
Ctrl-C in vite terminal
Ctrl-C in pg_doorman terminal
```

- [ ] **Step 11.5: DO NOT commit.**

---

## Task 12: Pre-commit gates (last sanity)

- [ ] **Step 12.1: Re-run all gates from `frontend/`**

```bash
cd /home/vadv/Projects/pg_doorman/frontend
npm run lint
npm run typecheck
rm -rf dist
npm run build
ls dist
```

Expected: lint and typecheck exit 0, build succeeds, `dist/index.html` and `dist/assets/` exist.

- [ ] **Step 12.2: Confirm no Rust files modified**

```bash
cd /home/vadv/Projects/pg_doorman
git status
```

Expected:
- Modified: nothing.
- Untracked: `frontend/`, `.github/workflows/frontend.yml`, plus the pre-existing untracked files (`.local/`, `Dockerfile.ubuntu22-tls`, `INCIDENT_*.md`).

If `Cargo.lock`, `Cargo.toml`, or any `src/**` is modified — investigate before committing.

- [ ] **Step 12.3: DO NOT commit yet.**

---

## Task 13: Pre-commit code review + single phase commit

- [ ] **Step 13.1: Stage Phase 5 files**

```bash
cd /home/vadv/Projects/pg_doorman
git add frontend/
git add .github/workflows/frontend.yml
git add docs/superpowers/plans/2026-05-06-web-ui-phase-5.md
git status
git diff --staged --stat | tail -20
```

Expected: `frontend/...` (many files including `dist/`), `.github/workflows/frontend.yml`, and the plan file staged.

- [ ] **Step 13.2: Draft commit message**

```
feat(web): land frontend skeleton (phase 5)

Adds frontend/ with Vite 6 + React 18 + TypeScript 5 + Tailwind v4 +
react-router 6, plus a CI workflow that lint/typecheck/build-checks
the bundle and fails when frontend/dist is out of sync with
frontend/src. Backend stays untouched; the bundle is not embedded
into the binary yet (phase 7).

The shell mounts AuthGate around six placeholder pages: Overview,
Pools, Clients, Caches, Logs, Config. AuthGate probes /api/version
on mount and renders a basic-auth modal whenever the probe returns
401; credentials live in React state and are lost on refresh.

Hooks usePoll and useAdminAuth ship empty of consumers so phase 6
pages have the primitives ready. Tailwind v4 @theme block transcribes
the design system tokens verbatim from
2026-05-06-web-ui-design-system.md; IBM Plex Sans/Mono is self-hosted
via @fontsource under the SIL Open Font License.

Frontend dist is committed to git; the CI job uses git diff
--exit-code frontend/dist as a regression guard so a developer who
forgets to rebuild gets a clear failure with the rebuild command.
RPM/DEB/Docker pipelines do not run npm.
```

- [ ] **Step 13.3: Run pre-commit reviewer (CLAUDE.md mandatory rule)**

Dispatch a general-purpose subagent (model: opus) with the standard pre-commit reviewer prompt from `~/.claude/CLAUDE.md`, passing the draft commit message inline. The reviewer will:
- Evaluate the diff (large — entire frontend/ scaffold).
- Load `frontend-design` and `stop-slop` skills as the diff touches frontend code and includes prose comments.
- Flag any blockers (slop in comments, language mismatch, slop in commit message, dead test code, etc.).

If reviewer returns "КОММИТ ЗАБЛОКИРОВАН", fix the listed blockers in the relevant task and re-run the reviewer.

- [ ] **Step 13.4: Commit**

```bash
cd /home/vadv/Projects/pg_doorman
git commit -m "$(cat <<'EOF'
[paste the final approved message from 13.2 here, possibly updated by reviewer feedback]
EOF
)"
```

(Heredoc keeps blank lines and avoids shell escaping issues.)

- [ ] **Step 13.5: Verify commit**

```bash
git log --oneline -3
git show --stat HEAD | head -20
```

Expected: latest commit is the phase 5 commit, includes the frontend/ tree + the workflow file.

- [ ] **Step 13.6: DO NOT push.** Wait for explicit user confirmation per project memory rule (`feedback_no_push_without_asking`).

---

## Self-Review

**Spec coverage** (sections from `2026-05-06-web-ui-phase-5-design.md`):

- §3 Stack — Task 1 (package.json), Task 2 (tsconfig + ESLint), Task 4 (Tailwind v4 entry).
- §4 Layout — Tasks 1–4 produce the listed files.
- §5 Components — Sidebar (Task 8), AuthGate (Task 6), App shell (Task 8).
- §6 Hooks — usePoll (Task 7), useAdminAuth (Task 5).
- §7 API client — Task 5 (`api.ts`, `types.ts`).
- §8 Build & dist — Task 9.
- §9 CI workflow — Task 10.
- §10 Design system mapping — Task 4 stylesheet, IBM Plex via @fontsource.
- §11 Out of scope — explicitly enumerated in plan header and not addressed.
- §12 Implementation phases — Tasks 1–8 mirror that ordering with one extra step per Task for build/dist (Task 9) and CI (Task 10) and smoke (Task 11).
- §13 Risks — addressed:
  - lockfile churn — accepted (Task 1.5).
  - Tailwind v4 maturity — no third-party plugins used.
  - dist size — Task 9.2 measures.
  - font licensing — already resolved (SIL OFL); Task 4.1 imports @fontsource packages.

All requirements have a task. No gaps.

**Placeholder scan:** No "TBD"/"TODO"/"implement later"/"add appropriate error handling"/"fill in details". One spot uses "if it does, add the suppressing eslint-disable comment with a justifying line" (Task 7.2) which describes a conditional remediation rather than a placeholder; left as-is because we can't predict whether eslint will fire.

**Type/name consistency:**
- `apiGet`, `Unauthorized`, `ApiError` — defined in Task 5.3, used in Task 6.1. ✓
- `AdminAuthProvider`, `useAdminAuth`, `Credentials` — Task 5.2; consumed in Task 8.3 (`<AdminAuthProvider>`) and Task 6.1 (`useAdminAuth()`). ✓
- `VersionDto` — Task 5.1; used in Task 6.1 typing. ✓
- `Sidebar` — Task 8.1 (named export), used in Task 8.3 with named import. ✓
- Page components — Task 8.2 default exports, imported as defaults in Task 8.3. ✓
- `usePoll` — Task 7.1 named export; not yet consumed (phase 6). ✓

**Commit policy:** Tasks 1–12 explicitly say "DO NOT commit". Task 13 produces the single phase commit. Matches phase 4 pattern.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-06-web-ui-phase-5.md`. Recommend **Subagent-Driven** execution with the standard subagent-driven-development workflow: fresh subagent per task, spec-compliance review then code-quality review between tasks. Most tasks are mechanical scaffolding (sonnet); Task 6 (AuthGate) and Task 8 (App shell + router wiring) get opus.
