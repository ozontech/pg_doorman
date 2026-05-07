# Web UI

pg_doorman ships a small operator console that runs from the same listener
as the Prometheus exporter. The bundle is embedded into the binary, so the
deployment story is identical to a UI-less build: one process, one binary,
one TCP port.

## Enabling

The UI lives under the `[web]` section of the config. The legacy
`[prometheus]` block is still accepted as an alias.

```toml
[web]
enabled = true
host = "0.0.0.0"
port = 9127

# Operator console (default off)
ui = true
ui_anonymous = false
log_tap_max_entries = 8192
```

`web.ui = true` is silently demoted to "metrics only" at startup when
`general.admin_password` is empty or the literal `"admin"`: the listener
keeps serving `/metrics`, but every admin-only endpoint would otherwise
be trivially open. Set a real password before flipping `ui = true`; you
will see `web.ui = true ignored: admin_password is default/empty` in
the log when this gate fires.

| Option | Description | Default |
|---|---|---|
| `enabled` | Whether the listener binds at all. `/metrics` works regardless of `ui`. | `false` |
| `host` | Bind address. | `"0.0.0.0"` |
| `port` | Bind port. | `9127` |
| `ui` | Serve the operator console on `/` and the public API endpoints. | `false` |
| `ui_anonymous` | When `true`, public API endpoints (`/api/version`, `/api/overview`, `/api/pools`, ...) accept unauthenticated requests. Admin endpoints (`/api/logs`, `/api/prepared/text/...`, `/api/interner/top`, `/api/admin/...`) always require basic auth. | `false` |
| `log_tap_max_entries` | Ring buffer size for the in-memory log tap powering `/api/logs`. Set to `0` to disable the endpoint. | `8192` |

## URL surface

| URL | Auth | Purpose |
|---|---|---|
| `/` and any non-API path | Always public when `web.ui` is active | The SPA shell. Browsing to `/pools` directly must not trigger a browser-native basic-auth dialog before the React sign-in modal can render — `ui_anonymous` does not gate the shell. |
| `/assets/*` | Always public when `web.ui` is active | Hashed JS / CSS / font bundles. Served with `Cache-Control: public, max-age=31536000, immutable`. |
| `/metrics` | None | Prometheus exposition format. Unaffected by `ui`. |
| `/api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/prepared`, `/api/apps`, `/api/events` | Public when `ui_anonymous = true`, otherwise admin | Read-only JSON. Field shapes mirror `SHOW <admin-command>`. |
| `/api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top`, `/api/top/queries` | Admin (basic auth) | Admin-only. `/api/logs` activates the in-memory tap on first request and self-disables after 2 minutes without traffic. `/api/top/queries` returns the first ~120 characters of cached SQL text — kept admin-only because previews can include literal values and tenant identifiers. |

## Authentication

The console uses HTTP basic auth with the `admin_username` / `admin_password`
credentials from `[general]`. The password is matched in constant time.
Browsers receive a `WWW-Authenticate: Basic` challenge on 401, so curl, gh,
and the like behave normally. Requests that advertise
`Accept: application/json` (the SPA's `fetch` wrapper) get a plain 401
without the challenge — without that, the browser caches whatever the
operator typed at the OS-level basic-auth dialog and replays it under the
SPA modal.

By default, credentials entered into the console live only in React state
and are lost on a hard refresh. Tick "Remember me on this device" in the
sign-in modal to persist them in the browser's `localStorage` so the
console survives a reload. Clearing the site's storage in the browser
wipes the entry.

## Pages

The SPA exposes:

- **Overview** — health pill, four golden-signal sparklines (latency p95,
  traffic, errors/s, saturation), connection breakdown stacked area,
  pool fill heatmap, dual-axis wait + oldest-active-age, top-5 errors
  per pool, and a collapsed resource detail panel.
- **Pools** — sortable table with mini-sparklines per row.
- **Pool detail** (`/pools/:poolId`) — full per-pool drill-down: SQLSTATE
  breakdown, oldest-active-age, pause/resume/reconnect controls.
- **Clients** — paginated table backed by `/api/clients` with server-side
  filter and sort.
- **Apps** — one row per `application_name` with err / 1k q ratio.
- **Caches** — Prepared statement table with hit rate, plus a query
  interner card (named vs anonymous bytes).
- **Logs** — live tail of the LogTap with level / target filter and
  pause / auto-scroll toggles.
- **Config & state** — collapsed panels covering `[general]` keys, the
  active log filter, `auth_query` cache, databases, users, sockets,
  pool scaling, pool coordinator.
- **War room** (`/wall`) — six huge tiles, optimized for an incident
  bridge or a wall display.

## Building from source

The frontend bundle is checked into git under `frontend/dist/` so that
RPM/DEB/Docker pipelines do not need a node toolchain. Developers editing
the SPA must rebuild before committing:

```bash
cd frontend
npm ci
npm run install-hooks   # one-time: wires the dist-sync pre-commit hook
npm run lint
npm run typecheck
npm run build
```

`npm run install-hooks` is opt-in. CI does not need it: the
`frontend.yml` workflow runs `npm run check-dist` and refuses to merge
when a commit changed source files without rebuilding `dist/`.

A separate `.github/workflows/frontend.yml` runs the same gates on every
PR that touches `frontend/`.

## Deployment

`/metrics` is unauthenticated on the same listener that can serve the
UI. That mirrors the historical Prometheus exporter and keeps existing
scrape configs working. If you put pg_doorman behind a reverse proxy,
remember that auth on `/api/*` does **not** propagate to `/metrics` —
metrics expose pool names, users, databases, connection pressure,
auth-query state, and workload shape. Either keep `[web]` on a private
host/port that only your scrape system reaches, or front the listener
with a proxy that adds auth on `/metrics` separately.
