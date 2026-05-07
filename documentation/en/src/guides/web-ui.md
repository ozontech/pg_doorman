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

## SSO and roles

The console enforces three access levels server-side. They apply
regardless of UI:

| Role | Activation | Access |
|---|---|---|
| `Anonymous` | no credentials and `ui_anonymous = true` | Public `/api/*` without personal data. Personal-data paths (`/api/logs`, `/api/prepared/text/...`, `/api/interner/top`, `/api/top/queries`) and `/api/admin/*` are denied. |
| `Sso` | a valid JWT in `Authorization: Bearer`, `Cookie: sso_access_token=...`, or query `?token=...` | Full read-only access including logs and SQL text. Mutating operations (`POST /api/admin/*`) are denied with `403 Forbidden` and body `{"error":"forbidden","message":"admin role required"}`. |
| `Admin` | the matching Basic credentials from `[general].admin_username` / `admin_password` | Everything, including `POST /api/admin/{reload,pause,resume,reconnect}`. |

When a request carries both Basic and an SSO token, Basic wins. A
correct admin password trumps any SSO token: the request resolves to
Admin. Broken Basic does not block a valid SSO token; the fallback
covers an expired token in `localStorage` next to a working Basic
password.

`401 Unauthorized` is returned when no credentials were sent or they
failed to parse. `403 Forbidden` is returned when credentials are
valid but the role is too low. The frontend re-opens the sign-in
modal on 401 and shows an "admin role required" banner on 403 instead
of opening the login screen.

### Enabling SSO

1. Obtain the RSA public key the SSO proxy uses to sign JWTs and write
   it to a file (e.g. `/etc/pg_doorman/sso-public.pem`). For
   `oauth2-proxy` extract it from the private key with
   `openssl rsa -in private.pem -pubout -out public.pem`. For Keycloak,
   copy the realm's RSA public key from Realm Settings → Keys.
2. Add the SSO fields to the `[web]` section:

   ```toml
   [web]
   enabled = true
   ui = true
   host = "127.0.0.1"
   port = 9127
   ui_anonymous = false

   sso_enabled = true
   sso_proxy_url = "https://sso.example.com/oauth2/start"
   sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
   sso_audience = ["pg_doorman"]
   sso_allowed_users = ["*"]
   ```

3. Reload the config: `kill -SIGHUP <pid>` or
   `psql -h <host> -p 6432 -U admin -d pgbouncer -c 'RELOAD'`.
4. Verify: `curl http://<host>:9127/api/auth/config` should return
   `"sso_enabled":true` and the configured `sso_proxy_url`.

| Field | Purpose | Default |
|---|---|---|
| `sso_enabled` | Turns the SSO branch on. JWTs are not validated when this is `false`. | `false` |
| `sso_proxy_url` | URL the SPA redirects the browser to for "Sign in via SSO". Server-side validation does not look at this field. | `null` |
| `sso_public_key_file` | Path to a PEM-encoded RSA public key. Read on start and on RELOAD. | `null` |
| `sso_audience` | Allowed `aud` claim values. A token passes when at least one matches. Required when `sso_enabled = true`. | `[]` |
| `sso_allowed_users` | Allowlist of `preferred_username` (or `sub`) claims. `["*"]` accepts everyone. Otherwise only the listed usernames pass. | `["*"]` |
| `sso_groups_claim` | JWT claim that lists the user's group memberships. Used together with `sso_admin_groups`. | `"groups"` |
| `sso_admin_groups` | Group names that promote an SSO user to Admin (full access, including `POST /api/admin/*`). Empty keeps SSO read-only. | `[]` |
| `trusted_proxies` | CIDR ranges trusted to set `X-Forwarded-For` / `Forwarded`. When the TCP peer falls in this list, the access log walks the proxy header to find the real client IP. Empty trusts only the listener's own peer. | `[]` |

If `sso_enabled = true` but `sso_public_key_file` is missing or the PEM
fails to load, the listener logs an error and silently keeps SSO off
for that run, so a misconfigured SSO section cannot take the operator
console down. The reason is exposed in two places:

- `/api/auth/config.sso_config_error` carries a human-readable
  message; the SPA renders a banner so the operator sees the
  rollout is broken instead of silently logging in via Basic.
- `pg_doorman_web_sso_config_error` Prometheus gauge stays at 1
  while the listener has SSO disabled despite the config asking for
  it. Pair with `pg_doorman_web_sso_enabled` to alert.

### SSO Admin via group claim

By default an SSO login resolves to the `Sso` role — read-only with
access to logs and SQL text, but no `POST /api/admin/*`. To let SSO
operators perform administrative operations without sharing the
Basic password, configure `sso_groups_claim` and `sso_admin_groups`:

```toml
[web]
sso_enabled = true
sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
sso_audience = ["pg_doorman"]
sso_groups_claim = "groups"
sso_admin_groups = ["pg-doorman-admins"]
```

When the validated JWT carries `"groups": [..., "pg-doorman-admins"]`,
pg_doorman resolves the request to `Admin` (with `source=sso`). The
SPA shows the same admin surface a Basic login would and the access
log records `auth_role=admin auth_source=sso`. Empty
`sso_admin_groups` (the default) keeps the SSO surface read-only.

### Real client IP behind a reverse proxy

When pg_doorman sits behind a reverse proxy, the access log's `peer`
field records the proxy's TCP address by default. To surface the
real client IP, list the proxy's CIDR in `[web].trusted_proxies`:

```toml
[web]
trusted_proxies = ["10.0.0.0/8", "192.168.0.0/16"]
```

The listener then parses `X-Forwarded-For` (or RFC 7239 `Forwarded`)
when the request peer is in the trusted list, walks the chain
right-to-left, skips any further trusted hops, and uses the first
untrusted address as `peer`. Untrusted clients can no longer spoof
the field — when the request peer is not in the trusted list, the
proxy headers are ignored.

### Signing in from the browser

On first load you see the sign-in modal. When `/api/auth/config`
returns an `sso_proxy_url`, the modal shows a **Sign in via SSO**
button alongside the Basic form. Clicking it sends the browser to
`${sso_proxy_url}?redirect_to=<current href>`. The proxy runs
OAuth/OIDC and redirects back with `?token=<jwt>`. The SPA stores the
token in `localStorage`, rewrites the URL clean of the parameter, and
sends the token on every later request.

The sidebar footer shows the resolved username: `admin` for Basic, or
`sso: <preferred_username>` for SSO. The sign-out button clears both
`pgdoorman.admin-auth` and `pgdoorman.sso-token` and re-opens the
sign-in modal.

Silent token refresh runs every 60 seconds. When the JWT is less than
90 seconds from `exp`, the SPA opens a hidden iframe at
`${origin}/?sso_silent=1`. The App router renders a minimal callback
component there (no normal polling effects) which posts the new token
back to the parent via `window.postMessage`. If silent refresh fails
and Basic is available, the SPA discards the SSO token without
redirecting; otherwise the SPA falls back to a full redirect through
the proxy. Configure JWT lifetime to at least 5 minutes; shorter
tokens may expire before the refresh fires.

### Access log

Every successful or auth-rejected response (200/401/403/404, including
`/metrics` scrapes) emits one logfmt line at info level via the
standard pg_doorman logger:

```
INFO pg_doorman::web::access method=GET path=/api/admin/reload query=false status=200 bytes=42 latency_ms=12 peer=10.0.1.5:42312 auth_role=admin auth_source=basic auth_user=admin
```

Fields: `method`, `path`, `query=true|false`, `status`, `bytes`,
`latency_ms`, `peer` (the TCP peer; when pg_doorman sits behind a
reverse proxy, this is the proxy's address), `auth_role`
(`admin`/`sso`/`anonymous`/`rejected`), `auth_source`
(`basic`/`sso`/`-`), and `auth_user`. Request and response bodies are
not logged; the query string is reduced to a presence flag so JWTs in
`?token=` never reach the log. The dedicated target
`pg_doorman::web::access` lets the LogTap target filter exclude the
access stream from `/api/logs`, or include only it.

### Troubleshooting

- **401 on a JWT that should be valid.** Confirm the token's `aud`
  matches one of the `sso_audience` values and that `exp` has not
  passed. Validate the PEM with `openssl rsa -pubin -in <pem> -text -noout`.
- **403 on a JWT that should be valid.** The path requires `Admin`
  (e.g. `POST /api/admin/reload`). SSO grants only read-only access.
- **Silent refresh does not fire.** Configure the SSO proxy to return
  the token without a login screen when the iframe carries an active
  session. With oauth2-proxy, set `--silent-refresh=true`.
- **Cookie-based JWT is ignored.** The cookie must reach pg_doorman
  on the same domain, and the `aud` claim must be in `sso_audience`.

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
