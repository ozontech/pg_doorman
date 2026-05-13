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
| `trusted_proxies` | CIDR ranges trusted to set `X-Forwarded-For` / `Forwarded` / `X-Forwarded-Proto`. Empty trusts only the listener's own peer. See [Access log](#access-log). | `[]` |

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

The sidebar lists eight routes; **War room** is reached from the
Overview hero, not from the nav. Pages that show SQL text or log lines
are gated to `Sso` and `Admin` — the sidebar hides their links for
anonymous viewers.

### Overview (`/overview`)

The default landing page. Polls `/api/overview` and `/api/pools` every
1.5 s. The header shows the health state (OK / DEGRADED / CRITICAL)
and a button that opens **War room**.

Tiles on the page:

- **Patroni-assisted fallback banner** — appears at the top of the
  page when any pool reports `fallback_active = true`. Lists every
  pool currently routing to the fallback backend and links to the
  [Patroni-assisted fallback tutorial](../tutorials/patroni-assisted-fallback.md).
  When the cooldown clears and pools return to the primary, the
  banner disappears on the next poll.
- **Golden-signal sparklines** — query p95, qps, errors/s,
  saturation. Each tile has a structured help popover with definition,
  source (the matching `SHOW` admin command), formula, thresholds,
  and a docs link.
- **Connection breakdown** — stacked area: active / idle / waiting
  clients across all pools.
- **Pool saturation heatmap** — one row per pool, 60 cells back,
  green/amber/red by `active / max_connections`.
- **Dual-axis wait + oldest-active-age** — companion charts: queued
  clients on one axis, age of the longest-running query on the other.
- **Top SQLSTATE codes** — aggregated error-code frequency across
  every pool since pg_doorman started. Each row shows the SQLSTATE,
  a short description, and the count. Open **Pool detail** for
  per-pool counts.
- **Resource detail (collapsed)** — process RSS, CPU, FDs, tokio
  thread balance, sockets, query interner. Collapsed by default; it is
  diagnostic detail, not an alert.

Admin actions are on the pages that own their scope. See
[Admin actions](#admin-actions).

Reads `SHOW POOLS`, `SHOW STATS`, `SHOW POOL_COORDINATOR`,
`SHOW POOL_SCALING`, `SHOW INTERNER`, `SHOW SOCKETS`. See the
[Admin commands reference](../observability/admin-commands.md).

### Pools (`/pools`)

Sortable table of every pool the daemon knows about. One row per
`user@database`. Columns: capacity, active, waiting, query p95, error
rate, saturation, fallback flag, plus a per-row mini-sparkline so a
ramping pool is visible without opening the detail page. Select a row
to open **Pool detail**.

### Pool detail (`/pools/:poolId`)

Full drill-down for one pool. The hero shows a paused badge when
`PAUSE` is in effect. Sections:

- **Configuration & state** — pool mode, sizes, timeouts, current
  active / idle / waiting counts. See
  [Pool modes](../concepts/pool-modes.md) for what the mode names
  mean.
- **TLS & fallback** — TLS state for backend connections, plus the
  `fallback_active` flag and a link into the
  [Patroni-assisted fallback tutorial](../tutorials/patroni-assisted-fallback.md).
- **Errors by SQLSTATE** — every SQLSTATE this pool has produced
  since start, sorted by count. Each row carries the short PostgreSQL
  description.
- **Startup parameters (operator-injected)** — only present when the
  config sets per-pool overrides. Each row shows
  `parameter = value`, the cascade source (`general` / `pool` /
  `auth_query` / `database`), and the state (`applied`,
  `dropped_due_to_budget`, etc.). Anonymous viewers see `***`
  instead of the value; the parameter name and source stay visible.
  Full semantics: [PostgreSQL startup parameters](../tutorials/startup-parameters.md).
- **Pool scaling** — backend creates, gate waits, budget exhaustions,
  anticipation hits, fallback creates. See
  [Pool Pressure (advanced)](../tutorials/pool-pressure.md).
- **Threshold reasons** — the health checks currently active for this
  pool.

The page hosts the per-pool **Admin actions** bar: PAUSE, RESUME,
RECONNECT, plus the global RELOAD. See [Admin actions](#admin-actions).

### Clients (`/clients`)

Paginated, polled view of every client connected to the pooler.
Filters live in the URL, so the current view can be copied into an
incident channel:

```
/clients?pool=shop_checkout&state=waiting&user=app
```

Filters: pool, database, user, state (active / idle / waiting /
closing), `application_name`, peer address. Sortable by queries,
errors, age, current-query age. Reads `SHOW CLIENTS`. Use it with
**Servers** to follow the client → backend hop: match `#cNNN` to a
`process_id`.

The page polls every 3 s and uses `React.memo` on rows: per-client
polling at 1.5 s kept Chrome memory growing under hundreds of
sessions.

### Servers (`/servers`)

Paginated, polled view of every backend connection pg_doorman
currently holds. Reads `SHOW SERVERS`. URL filters: database, user,
state (active / idle / used / login), `application_name`. Each row
shows `server_id`, `process_id` (the PostgreSQL pid), the
user@database pair, application, state, active-query age, queries
and errors served, bytes sent/received, and TLS flag.

Use this together with **Clients** to map a stuck query: take the
`server_id` from the client row, open this page, and look up the pid in
`pg_stat_activity`.

### Apps (`/apps`)

One row per `application_name` from the libpq `StartupMessage`.
Derived from `SHOW CLIENTS` grouped on the backend. Browser-side
sort and filter. Each row shows live clients, qps, tps, total
queries / transactions / errors, and an `err / 1k q` ratio.

### Caches (`/caches`)

Two tabs:

- **Prepared** — per-pool prepared-statement cache (hash → `DOORMAN_N`)
  with hit rate. See [Anonymous Parse Caching](../tutorials/prepared-statements.md).
- **Query cache** — process-wide SQL text interner (named +
  anonymous bytes). Reads `SHOW INTERNER` and `SHOW INTERNER <N>`.

Both tabs show SQL text and are personal-data paths: the sidebar
link is hidden for anonymous viewers, and the API endpoint returns
`401` without the `Sso` or `Admin` role.

### Logs (`/logs`)

Live tail of the LogTap side-channel. The tap activates on the first
`/api/logs` request and self-disables after 2 minutes without traffic,
so a closed tab does not keep the ring buffer active.

Filters live in the URL: `level`, `q` (substring), `paused`, `scroll`.
The filtered URL survives refresh and can be shared:

```
/logs?level=ERROR&q=53300
```

Pause freezes the on-screen view (the backend ring buffer keeps
filling). The footer shows tap state, used/capacity, and drops since
the tap opened. Personal-data path: `Sso` / `Admin` only.

If `[web].log_tap_max_entries = 0`, the page renders an instruction
panel instead of an empty stream — log streaming is off in the
running config until that value is raised and pg_doorman is restarted.

### Config & state (`/config`)

Read-only mirror of `SHOW CONFIG`, `SHOW DATABASES`, `SHOW USERS`,
`SHOW AUTH_QUERY`, `SHOW LOG_LEVEL`, `SHOW STARTUP_PARAMETERS`,
`SHOW SOCKETS`, `SHOW POOL_SCALING`, `SHOW POOL_COORDINATOR`. Each
section is a collapsible panel.

The "startup parameters" panel lists every pool × override pair from
the cascade — useful when checking a TOML change before reloading.
The Reload-able column on the config panel marks which keys take
effect on `RELOAD` versus which require a restart.

A global **Reload config** button is in the page header (admin role
only). It sends the same `POST /api/admin/reload` as the psql admin
console, behind a typed `RELOAD` confirmation. See
[Admin actions](#admin-actions).

### War room (`/wall`)

Large-screen view of the Overview data. The sidebar and helper
popovers are hidden; six KPI tiles (max p95, errors/s, max saturation,
waiting, oldest active, pool count) sit under a full-width pool
saturation heatmap. A red border pulses when any signal crosses its
critical threshold. Recent admin events are listed at the bottom so a
metric spike can be matched with the action that preceded it.

The page acquires a `navigator.wakeLock("screen")` so the TV does
not blank, and exits to `/overview` on **Esc**. It opens from the
Overview header ("Open war room") because it is the Overview data in a
large-screen layout.

## Admin actions

The four mutating admin operations from the
[admin commands reference](../observability/admin-commands.md) are
exposed in the SPA. Each requires the `Admin` role and a typed
confirmation that matches the underlying command:

| Action       | Scope                  | Where to find it                                                        | Confirmation phrase |
|--------------|------------------------|-------------------------------------------------------------------------|---------------------|
| `RELOAD`     | every pool             | Config & state hero · Pool detail action bar                            | `RELOAD`            |
| `PAUSE`      | single `user@database` | Pool detail action bar                                                  | the database name   |
| `RESUME`     | single `user@database` | Pool detail action bar (enabled only when the pool is currently paused) | the database name   |
| `RECONNECT` | single `user@database` | Pool detail action bar                                                  | the database name   |

The same semantics apply as in the psql admin protocol. `PAUSE` stops
new checkouts on the targeted pool; in-flight transactions keep
running. `RESUME` re-enables checkouts. `RECONNECT` drops idle
backends and refuses active ones when they return — use it after a
PostgreSQL role or grant change so cached connections pick the new
state up. `RELOAD` re-reads `pg_doorman.toml`; pool sizes shrink via
natural drain. Typed confirmation prevents accidental clicks during an
incident: `RELOAD` touches every pool, and `PAUSE` on the wrong pool
can stop unrelated traffic.

After a successful action, the UI shows a top-right toast, for example
"Config reload requested" or "PAUSE applied to `<db>`". Failures show
an error toast with the response body. Admin
actions also write `info`-level lines to the access log
(`auth_role=admin auth_source=basic|sso`) and append to the
admin-event buffer. **Overview** marks the action with a vertical
annotation; **War room** lists it under "Recent admin events".

## Keyboard shortcuts

Shortcuts work anywhere outside text fields. Press **?** to open the
in-app shortcut list.

| Combo              | Effect                                                                |
|--------------------|-----------------------------------------------------------------------|
| <kbd>⌘ K</kbd> / <kbd>Ctrl K</kbd> | Open the command palette: jump to a page or find a pool by id, database, or user, then press **Enter**. |
| <kbd>?</kbd>       | Open the keyboard shortcut modal.                                     |
| <kbd>Esc</kbd>     | Close a popover or modal. On `/wall`, return to **Overview**.         |

The command palette polls `/api/pools` whenever it opens, so the pool
list is up to date instead of coming from page load.

## Theme

A three-position toggle in the sidebar footer picks **Light** /
**System** / **Dark**. The default is **Light** — the dark theme is
opt-in. The choice persists in `localStorage`. **System** tracks the
OS / browser preference and switches automatically when the OS
flips between light and dark mode.

The palette uses Geist Sans for interface text and JetBrains Mono for
numeric and identifier columns; accent is `#2563eb` on light,
`#60a5fa` on dark.

## In-app help

Every metric tile and section header carries a small (i) icon next
to the title. Click or hover to open a help popover:

- **Definition** — one sentence on what the metric is.
- **Source** — the admin SQL command behind the number, e.g. `SHOW
  POOLS`, `SHOW STATS`, `SHOW PREPARED_STATEMENTS`.
- **Formula** — the computation expression, when one exists.
- **Thresholds** — green / amber / red bands with concrete numbers.
- **Related** — other metric names that appear elsewhere in the
  console.
- **Open in docs** — link into this guide or one of the tutorials.

The layout is consistent across pages, so the same fields mean the
same thing on **Overview**, **Pool detail**, and **Caches**.

## Toasts

The UI shows Sonner toasts in the top-right corner. Admin actions and
non-fatal errors use them; the rest of the page keeps running while
the message is visible. Each toast stays on screen for 4 s. The
Toaster follows the active theme.

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
