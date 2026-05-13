# Web UI

pg_doorman ships a single-page operator console that runs on the same listener
as the Prometheus exporter. The frontend bundle is embedded in the binary, so
the deployment story is identical to a UI-less build: one process, one binary,
one TCP port.

## Enabling

The console lives under the `[web]` section of the config. The legacy
`[prometheus]` block name is still accepted as an alias.

```toml
[web]
enabled = true
host = "0.0.0.0"
port = 9127

# Operator console (off by default)
ui = true
ui_anonymous = false
log_tap_max_entries = 8192
```

`web.ui = true` is silently demoted to "metrics only" at startup when
`general.admin_password` is empty or the literal `"admin"`. The listener
keeps serving `/metrics`, but every admin-only endpoint would otherwise
be trivially open. Set a real password before flipping `ui = true`. The
log line `web.ui = true ignored: admin_password is default/empty` confirms
this gate fired.

| Option | Description | Default |
|---|---|---|
| `enabled` | Whether the listener binds at all. `/metrics` works regardless of `ui`. | `false` |
| `host` | Bind address. | `"0.0.0.0"` |
| `port` | Bind port. | `9127` |
| `ui` | Serve the SPA on `/` and the public API endpoints. | `false` |
| `ui_anonymous` | When `true`, public API endpoints accept unauthenticated requests. See [Access roles](#access-roles). | `false` |
| `log_tap_max_entries` | Ring-buffer size for the in-memory log tap behind `/api/logs`. `0` disables the endpoint. | `8192` |

## URL endpoints

| URL | Required role | Purpose |
|---|---|---|
| `/`, `/pools`, any non-API path | none | The SPA shell. Served anonymously even when `ui_anonymous = false`, so deep links do not trip a browser-native Basic-auth dialog before the React sign-in modal can render. |
| `/assets/*` | none | Hashed JS, CSS, font, and SVG bundles. Served with `Cache-Control: public, max-age=31536000, immutable`. |
| `/metrics` | none | Prometheus exposition format. Unaffected by `ui`. |
| `GET /api/auth/config` | none | Tells the SPA whether SSO is wired and what role the current request holds. |
| `GET /api/version`, `/api/overview`, `/api/pools`, `/api/clients`, `/api/servers`, `/api/connections`, `/api/stats`, `/api/databases`, `/api/users`, `/api/auth_query`, `/api/config`, `/api/log_level`, `/api/pool_coordinator`, `/api/pool_scaling`, `/api/sockets`, `/api/prepared`, `/api/interner`, `/api/top/clients`, `/api/top/prepared`, `/api/apps`, `/api/events` | `Anonymous` when `ui_anonymous = true`, otherwise `Sso` | Read-only JSON that mirrors the `SHOW <admin-command>` shape. |
| `GET /api/logs`, `/api/prepared/text/{hash}`, `/api/interner/top`, `/api/top/queries` | `Sso` | Read-only personal-data endpoints. `/api/logs` activates the in-memory tap on first request and self-disables after 2 minutes without traffic. `/api/top/queries` returns the first ~120 characters of cached SQL text and is not available anonymously because previews can carry literal values and tenant identifiers. |
| `POST /api/admin/{reload,pause,resume,reconnect}` | `Admin` | Mutating admin actions. Same semantics as the psql admin protocol. |

## Access roles

The listener resolves every request to one of three roles. The role check runs
on the server; the SPA mirrors it on the client only to hide controls the
operator cannot use.

| Role | How the request earns it | What the role grants |
|---|---|---|
| `Anonymous` | No credentials, and `[web].ui_anonymous = true`. | Public read-only `/api/*` endpoints listed above, plus `/metrics`. Personal-data paths and `/api/admin/*` return `401`. |
| `Sso` | A valid JWT in `Authorization: Bearer`, in cookie `sso_access_token=`, or in query `?token=`, that does **not** match an admin group. | All read endpoints, including personal-data paths. `POST /api/admin/*` returns `403`. |
| `Admin` | Either a correct Basic credential pair against `[general].admin_username`/`admin_password`, or a valid JWT whose `[web].sso_groups_claim` value intersects `[web].sso_admin_groups`. | Everything, including `POST /api/admin/{reload,pause,resume,reconnect}`. |

When a request carries both Basic and an SSO token, the listener prefers
Basic. A correct admin password resolves to `Admin` regardless of any SSO
state. A wrong Basic password does not block the SSO branch: the SSO
sources still validate, and a valid JWT resolves to `Sso` (or `Admin`,
depending on the group claim). This covers the common case of a stale
JWT in `localStorage` next to a working Basic password.

The Basic password compare runs in constant time relative to the configured
credentials. JWTs are validated against the public key in
`[web].sso_public_key_file`; the listener caches the parsed key for the
process lifetime and reloads it on `RELOAD`.

The SPA `fetch` wrapper sends `Accept: application/json`, which makes the
listener emit a plain `401` without `WWW-Authenticate: Basic`. Without that,
the browser would cache whatever the operator typed in its native Basic
dialog and replay it on top of the React sign-in modal. Tools that send
`Accept: */*` (curl, gh) still receive the challenge and behave normally.

`401 Unauthorized` is returned when no credentials reached the listener
or every credential failed to parse or validate. `403 Forbidden` is
returned when credentials validated but the resolved role is too low for
the path; the body is `{"error":"forbidden","message":"admin role required"}`.
The SPA re-opens the sign-in modal on `401` and shows a non-blocking
"admin role required" banner on `403`.

## Configuring SSO

SSO is opt-in. With `[web].sso_enabled = false` (the default), the listener
serves only the Anonymous and Admin (Basic) roles. To wire an external SSO
proxy:

1. Obtain the RSA public key the proxy uses to sign JWTs and store it in a
   PEM file (e.g. `/etc/pg_doorman/sso-public.pem`). For oauth2-proxy,
   extract it from the private key with
   `openssl rsa -in private.pem -pubout -out public.pem`. For Keycloak,
   see [Keycloak](#keycloak) below.
2. Add the SSO fields to `[web]`:

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

3. Reload the config with `kill -SIGHUP <pid>` or
   `psql -h <host> -p 6432 -U admin -d pgbouncer -c 'RELOAD'`.
4. Verify with `curl http://<host>:9127/api/auth/config`. The response
   should carry `"sso_enabled":true` and the configured `sso_proxy_url`.

| Field | Purpose | Default |
|---|---|---|
| `sso_enabled` | Turns the SSO branch on. JWTs are not validated when this is `false`. | `false` |
| `sso_proxy_url` | URL the SPA redirects the browser to for "Sign in via SSO". The backend never calls this URL itself. | `null` |
| `sso_public_key_file` | Path to a PEM-encoded RSA public key. Read on start and on `RELOAD`. | `null` |
| `sso_audience` | Allowed `aud` claim values. A token passes when at least one matches. Required when `sso_enabled = true`. | `[]` |
| `sso_allowed_users` | Allowlist on the `preferred_username` (or `sub`) claim. `["*"]` accepts every valid JWT; a literal list restricts access to those usernames. | `["*"]` |
| `sso_groups_claim` | Name of the JWT claim that carries the user's group memberships. Read together with `sso_admin_groups`. | `"groups"` |
| `sso_admin_groups` | Group names that promote an SSO user to `Admin`. Empty keeps every SSO login at the read-only `Sso` role. | `[]` |
| `sso_require_https` | Reject Bearer/cookie/query SSO credentials presented over plain HTTP. The listener treats a request as secure only when the TCP peer is in `trusted_proxies` and `X-Forwarded-Proto: https` is forwarded. Defaults to off so SSO keeps working through a TLS-terminating proxy that reaches pg_doorman over a private HTTP leg. | `false` |
| `trusted_proxies` | CIDR ranges trusted to set `X-Forwarded-For` / `Forwarded` / `X-Forwarded-Proto`. With an empty list, pg_doorman ignores forwarded headers and uses the TCP peer address. If `sso_require_https = true` is behind a TLS-terminating proxy, add that proxy CIDR so `X-Forwarded-Proto: https` is accepted. See [Access log](#access-log). | `[]` |

### Promoting SSO users to Admin via group claim

By default an SSO login lands in `Sso` — read-only with access to logs and
SQL text, but no `POST /api/admin/*`. To let SSO operators run mutating
admin actions without sharing the Basic password, configure
`sso_groups_claim` and `sso_admin_groups`:

```toml
[web]
sso_enabled = true
sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
sso_audience = ["pg_doorman"]
sso_groups_claim = "groups"
sso_admin_groups = ["pg-doorman-admins"]
```

When the validated JWT carries `"groups": [..., "pg-doorman-admins"]`,
the request resolves to `Admin`. The access log records the promotion as
`auth_role=admin auth_source=sso`, so SSO admins are still distinguishable
from Basic admins. `/api/auth/config` reports
`sso_admin_groups_configured = true`, which lets the SPA stop promising
"SSO grants read-only access" in the sign-in modal.

### Keycloak

Keycloak signs every JWT with the realm's RSA key. Export the public
half once per realm into a PEM file pg_doorman can read.

The non-interactive way uses the realm's JWKS endpoint:

```bash
REALM=https://kc.example.com/realms/operators
curl -s "$REALM/protocol/openid-connect/certs" \
  | jq -r '.keys[] | select(.alg=="RS256") | "-----BEGIN CERTIFICATE-----\n" + .x5c[0] + "\n-----END CERTIFICATE-----"' \
  | openssl x509 -pubkey -noout \
  > /etc/pg_doorman/sso-public.pem
```

Or copy it from the admin UI: **Realm settings** → **Keys** → row with
`Algorithm = RS256` and `Use = SIG` → **Public key** → wrap the
copied base64 body into a `-----BEGIN PUBLIC KEY-----` PEM file.

A Keycloak-backed `[web]` section then looks like this:

```toml
[web]
sso_enabled = true
sso_proxy_url = "https://kc.example.com/realms/operators/protocol/openid-connect/auth"
sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
sso_audience = ["pg_doorman"]    # client_id configured on Keycloak
sso_groups_claim = "groups"      # default with the "groups" mapper enabled
sso_admin_groups = ["pg-doorman-admins"]
```

For Admin via group claim to work, add a **Group Membership** mapper
to the client (Clients → your client → **Mappers**). Without that
mapper Keycloak issues tokens without `groups`, and every operator
stays on `Sso`.

When Keycloak rotates the realm signing key, refetch the PEM and
issue `RELOAD`. pg_doorman picks the new key up without a restart.

### When SSO config is broken

A typo in the SSO section never knocks the operator console offline. When
`sso_enabled = true` but the runtime cannot load (missing PEM file, empty
audience, unparsable PEM), the listener logs the reason at `error` level,
keeps SSO disabled for that run, and serves only Basic and Anonymous
requests. The same reason is shown in two places so an operator notices
the broken rollout instead of silently falling back:

- `/api/auth/config.sso_config_error` carries a human-readable message.
  The SPA renders a banner with that text in the sign-in modal.
- The `pg_doorman_web_sso_config_error` Prometheus gauge stays at `1`
  while SSO is asked-for but not loaded. Pair it with
  `pg_doorman_web_sso_enabled` to alert.

## Browser sign-in flow

On first load the SPA fetches `/api/auth/config` and renders the sign-in
modal. When the response carries `sso_proxy_url`, the modal shows a
**Sign in via SSO** button next to the Basic form; otherwise only the
Basic form appears.

Clicking **Sign in via SSO** sends the browser to
`${sso_proxy_url}?redirect_to=<current href>`. The proxy runs the
OAuth/OIDC flow and bounces the browser back with `?token=<jwt>`. The
SPA stores the token in `localStorage`, rewrites the URL clean of the
parameter, and sends `Authorization: Bearer <jwt>` on every later
request.

The sidebar footer shows the resolved username: `admin` for Basic, or
`sso: <preferred_username>` for SSO. **Sign out** clears both
`pgdoorman.admin-auth` and `pgdoorman.sso-token` from `localStorage`
and re-opens the sign-in modal.

A silent-refresh poller wakes every 60 seconds. When the JWT is less
than 90 seconds from `exp`, the SPA opens a hidden iframe at
`${origin}/?sso_silent=1`. The App router renders a minimal
`SilentCallback` component there (no normal polling effects), which
posts the new token to the parent via `window.postMessage`. If silent
refresh fails:

- when a Basic credential is also present, the SPA discards the SSO
  token without redirecting and falls back to Basic for further
  requests;
- otherwise the SPA performs a full redirect through the SSO proxy.

Configure JWT lifetime to at least 5 minutes; tokens shorter than that
may expire before the refresh fires.

The SPA never sends cookies (`credentials: "omit"` on every fetch). The
`sso_access_token` cookie path exists for sidecars, curl, and
oauth2-proxy variants that paste the token into a cookie on the
shared domain.

The Basic credential lives only in React state by default and is lost
on a hard refresh. **Remember me on this device** in the sign-in modal
persists it in `localStorage` so the console survives a reload.
Clearing site storage in the browser wipes both the Basic and the SSO
entry.

## Access log

Every response (200/401/403/404/5xx, `/metrics` scrapes included) emits
one logfmt line on the `pg_doorman::web::access` target:

```
INFO pg_doorman::web::access method=GET path=/api/admin/reload query=false status=200 bytes=42 latency_ms=12 peer=10.0.1.5:42312 auth_role=admin auth_source=basic auth_user=admin
```

Fields:

- `method`, `path` — verb and URL path. Bodies are not logged.
- `query=true|false` — whether the request carried a query string. The
  string itself is reduced to a presence flag so JWTs in `?token=`
  never reach the log.
- `status`, `bytes`, `latency_ms` — response status, body size, and
  end-to-end latency.
- `peer` — the request peer address. By default this is the TCP peer.
  When the TCP peer falls in `[web].trusted_proxies`, the listener
  parses `X-Forwarded-For` (or `Forwarded`, RFC 7239), walks right to
  left skipping any further trusted hops, and uses the first untrusted
  address as `peer`. An untrusted client cannot spoof the field — the
  proxy headers are ignored when the peer is not trusted.
- `auth_role` — `admin`, `sso`, `anonymous`, or `rejected`.
- `auth_source` — `basic`, `sso`, or `-`.
- `auth_user` — resolved username, or `-` for anonymous and rejected.

Levels:

- `info` — every admin action (`POST /api/admin/*`), every
  personal-data read (`/api/logs`, `/api/prepared/text/*`,
  `/api/interner/top`, `/api/top/queries`), every auth/SSO endpoint
  (`/api/auth/*`, `/api/sso/*`), and every non-2xx response.
- `debug` — every other successful 2xx read, anonymous or
  authenticated. The SPA polls `/api/overview`, `/api/pools`,
  `/api/clients`, `/api/process` every 1.5–3 s; with the previous
  rule that every authenticated 2xx was `info`, an operator sitting
  on the Logs page saw their own polls. Routine reads are logged at
  `debug`, so `RUST_LOG=info` is limited to admin actions, auth
  traffic, and failures.

The dedicated `pg_doorman::web::access` target lets operators filter
the access feed independently of the rest of the logger. The LogTap
filter dropdown in the **Logs** page can include or exclude this
target with one click.

### Real client IP behind a reverse proxy

By default `peer` records the TCP address that connected to the
listener, which is the proxy when pg_doorman sits behind one. List
the proxy's CIDR in `[web].trusted_proxies` to record the real
client IP:

```toml
[web]
trusted_proxies = ["10.0.0.0/8", "192.168.0.0/16"]
```

Both `X-Forwarded-For` and `Forwarded` are recognised. Multiple
trusted hops in the chain are skipped. An untrusted client that
sends `X-Forwarded-For` is ignored, so this knob does not give
arbitrary callers control over the access-log field.

## Metrics

| Metric | Type | Labels | Purpose |
|---|---|---|---|
| `pg_doorman_web_sso_enabled` | gauge | — | `1` when SSO loaded successfully, `0` otherwise. |
| `pg_doorman_web_sso_config_error` | gauge | — | `1` when `sso_enabled = true` but the runtime failed to load. |
| `pg_doorman_web_auth_attempts_total` | counter | `role`, `source` | Authentication attempts by resolved role (`admin`/`sso`/`anonymous`/`rejected`) and source (`basic`/`sso`/`none`). |
| `pg_doorman_web_requests_total` | counter | `status_class`, `role` | Web requests by HTTP status class (`1xx`–`5xx`) and resolved role. |
| `pg_doorman_web_sso_validation_errors_total` | counter | `reason` | JWT validation failures by reason: `signature`, `expired`, `audience`, `no_username`, `allowlist`. |

A sustained spike in `signature` means the SSO proxy rotated keys without
updating `sso_public_key_file`. A spike in `allowlist` means a JWT outside
`sso_allowed_users` is repeatedly trying to log in. A spike in `4xx` for
the `sso` role usually points at a broken proxy in front of pg_doorman.

## Troubleshooting

**`401` on a JWT that should be valid.** Check that `aud` matches one of
the `sso_audience` values and that `exp` has not passed. Validate the
PEM with `openssl rsa -pubin -in <pem> -text -noout`. The
`pg_doorman_web_sso_validation_errors_total{reason}` counter shows which
check failed.

**`403` on a JWT that should be valid.** The path requires `Admin` (e.g.
`POST /api/admin/reload`). Either log in with the Basic admin password,
or add the user's group to `[web].sso_admin_groups` and reload the
config.

**SPA never offers Sign in via SSO.** `/api/auth/config` is not
returning `sso_proxy_url`. Either `[web].sso_enabled = false`, or
`sso_proxy_url` is unset, or the runtime failed to load (look for
`sso_config_error` in the same response).

**Silent refresh does not fire.** The SSO proxy must return a fresh
token without rendering a login screen when the iframe carries an
active session. With oauth2-proxy, set `--silent-refresh=true`.

**Cookie-based JWT is ignored.** The cookie must reach pg_doorman on
the same domain, and `aud` must be in `sso_audience`. The SPA itself
sends no cookies; cookie auth targets curl, sidecars, and oauth2-proxy
variants that forward the token via cookie on the shared domain.

## Pages

The sidebar has eight routes. **War room** opens from **Overview**.
Pages that expose SQL text or logs require the `Sso` or `Admin` role
and are hidden from anonymous users.

### Overview (`/overview`)

Default page for pooler health: main metrics, queues, pool saturation,
common SQLSTATE codes, and a collapsed resource block. If Patroni
fallback is active, a banner lists the affected pools.

### Pools (`/pools`)

Table of all `user@database` pools: size, active connections, waiting
clients, p95, errors, saturation, and fallback state. Selecting a row
opens **Pool detail**.

### Pool detail (`/pools/:poolId`)

Single-pool view: mode, limits, current connections, TLS, fallback
state, SQLSTATE counts, PostgreSQL startup parameters, and active
threshold reasons. Pool actions `PAUSE`, `RESUME`, `RECONNECT`, and
global `RELOAD` are available here.

### Clients (`/clients`)

Client table with URL filters:

```
/clients?pool=shop_checkout&state=waiting&user=app
```

Filters cover pool, database, user, state, `application_name`, and
peer address. Sorting covers queries, errors, connection age, and
current-query age. Use it with **Servers** to map a client to a
PostgreSQL pid.

### Servers (`/servers`)

Backend connections from `SHOW SERVERS`: `server_id`, `process_id`,
database, user, application, state, active-query age, counters, traffic,
and TLS. Use a client's `server_id` here to find the pid in
`pg_stat_activity`.

### Apps (`/apps`)

One row per `application_name`: active clients, qps, tps, totals, and
`err / 1k q`.

### Caches (`/caches`)

Prepared-statement cache by pool and process-wide SQL-text cache. Both
can show SQL text, so both require `Sso` or `Admin`.

### Logs (`/logs`)

LogTap stream with URL filters:

```
/logs?level=ERROR&q=53300
```

Pause freezes the view only; the server buffer keeps filling. If
`[web].log_tap_max_entries = 0`, the page reports that log streaming is
disabled. Access requires `Sso` or `Admin`.

### Config & state (`/config`)

Read-only mirror of `SHOW CONFIG`, `SHOW DATABASES`, `SHOW USERS`,
`SHOW AUTH_QUERY`, `SHOW LOG_LEVEL`, `SHOW STARTUP_PARAMETERS`,
`SHOW SOCKETS`, `SHOW POOL_SCALING`, and `SHOW POOL_COORDINATOR`.
It shows which config keys apply on `RELOAD` and which require restart.
**Reload config** is available only to `Admin`.

### War room (`/wall`)

Large-screen **Overview**: pool saturation, big metrics, and recent
admin actions. **Esc** returns to `/overview`.

## Admin actions

The SPA exposes four mutating operations:

| Action       | Scope                  | Where                            | Confirmation |
|--------------|------------------------|----------------------------------|--------------|
| `RELOAD`     | every pool             | Config & state · Pool detail     | `RELOAD`     |
| `PAUSE`      | one `user@database`    | Pool detail                      | database     |
| `RESUME`     | one `user@database`    | Pool detail, when paused         | database     |
| `RECONNECT` | one `user@database`    | Pool detail                      | database     |

Semantics match the psql admin protocol. `PAUSE` stops new backend
checkouts for the pool; in-flight transactions continue. `RESUME`
allows checkouts again. `RECONNECT` closes idle backends and rejects
active ones when they return. `RELOAD` re-reads `pg_doorman.toml`;
pool size shrinks as connections drain.

Typed confirmation protects against accidental `RELOAD` or `PAUSE` on
the wrong pool. Each action shows a result message, writes an `info`
access-log line, and appears in the recent admin-event list.

## Keyboard shortcuts

Shortcuts work outside text fields.

| Combo                                | Effect                                      |
|--------------------------------------|---------------------------------------------|
| <kbd>⌘ K</kbd> / <kbd>Ctrl K</kbd>   | Search pages and pools.                     |
| <kbd>?</kbd>                         | Show keyboard shortcuts.                    |
| <kbd>Esc</kbd>                       | Close help or modal. On `/wall`, go back.   |

## Theme

The sidebar footer has **Light** / **System** / **Dark**. Default is
**Light**. The choice is stored in `localStorage`.

## In-app help

Metric and section headers have an (i) icon. Help explains what the
number means, where it comes from, how it is calculated, and which
thresholds are normal.

## Building from source

The frontend bundle is checked into git under `frontend/dist/` so RPM,
DEB, and Docker pipelines do not need a node toolchain. Developers
editing the SPA must rebuild before committing:

```bash
cd frontend
npm ci
npm run install-hooks   # one-time: wires the dist-sync pre-commit hook
npm run lint
npm run typecheck
npm run build
```

`npm run install-hooks` is opt-in. CI does not need it: the
`.github/workflows/frontend.yml` workflow runs `npm run check-dist` and
refuses to merge when a commit changed source files without rebuilding
`dist/`. The same workflow runs lint and typecheck on every PR that
touches `frontend/`.

## Deployment

`/metrics` is unauthenticated on the same listener that serves the UI.
This mirrors the historical Prometheus exporter and keeps existing
scrape configs working. Auth on `/api/*` does **not** propagate to
`/metrics` — the metrics endpoint exposes pool names, users, databases,
connection pressure, auth-query state, and workload shape. Either bind
`[web]` to a private host/port that only your scrape system reaches,
or front the listener with a proxy that adds auth on `/metrics`
separately.
