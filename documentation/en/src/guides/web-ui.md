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

`web.ui = true` is refused at startup when `general.admin_password` is empty
or the literal `"admin"` — every admin-only endpoint would otherwise be
trivially open. Set a real password before flipping `ui = true`.

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
| `/` and any non-API path | Public when `ui_anonymous = true`, otherwise basic-auth | The SPA shell. Client-side routing is handled by the bundle, so deep links like `/pools` resolve to the same shell on hard refresh. |
| `/assets/*` | Same as `/` | Hashed JS / CSS / font bundles. Served with `Cache-Control: public, max-age=31536000, immutable`. |
| `/metrics` | None | Prometheus exposition format. Unaffected by `ui`. |
| `/api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/queries`, `/api/top/prepared`, `/api/apps`, `/api/events` | Public when `ui_anonymous = true`, otherwise admin | Read-only JSON. Field shapes mirror `SHOW <admin-command>`. |
| `/api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top` | Admin (basic auth) | Admin-only. `/api/logs` activates the in-memory tap on first request and self-disables after 30 s without traffic. |

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

The SPA renders six pages:

- **Overview** — health pill, four golden-signal sparklines (latency p95,
  traffic, errors/s, saturation), connection breakdown stacked area,
  pool fill heatmap, dual-axis wait + oldest-active-age, top-5 errors
  per pool, and a collapsed resource detail panel.
- **Pools** — sortable table with mini-sparklines per row and a click-out
  drawer with the full per-pool detail.
- **Clients** — paginated table backed by `/api/clients` with server-side
  filter and sort.
- **Caches** — Prepared statement table with hit rate, plus a query
  interner card (named vs anonymous bytes).
- **Logs** — live tail of the LogTap with level / target filter and
  pause / auto-scroll toggles.
- **Config** — eight collapsed panels covering `[general]` keys, the
  active log filter, `auth_query` cache, databases, users, sockets,
  pool scaling, pool coordinator.

## Building from source

The frontend bundle is checked into git under `frontend/dist/` so that
RPM/DEB/Docker pipelines do not need a node toolchain. Developers editing
the SPA must rebuild before committing:

```bash
cd frontend
npm ci
npm run lint
npm run typecheck
npm run build
```

A separate `.github/workflows/frontend.yml` runs the same gates on every
PR that touches `frontend/`.
