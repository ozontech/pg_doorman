# Changelog

### 3.10.7

#### pgjdbc LargeObject fastpath calls work in transaction pooling

pg_doorman now forwards PostgreSQL `FunctionCall` protocol messages and
passes `FunctionCallResponse` through until `ReadyForQuery` reports the
transaction state. The pgjdbc LargeObject API uses this fastpath path, and
transaction-mode clients could previously hang on `getLargeObjectAPI()`
because the frontend `F` message was not forwarded.

Regression coverage now includes wire-level fastpath checks in transaction
and autocommit pooling, plus a pgjdbc LargeObject round trip with a multi-MiB
payload.

### 3.10.6

#### Binary upgrade no longer carries migrated client fds into the next generation

Client fds received over the SIGUSR2 migration socket are now marked
close-on-exec in the new process. A chained binary upgrade used to inherit
stale copies of already-migrated client sockets, so every generation could
start with extra fds and eventually fail with `Too many open files` under
load.

The foreground upgrade path also marks inherited service fds close-on-exec
after startup and cleans up unexpected inherited descriptors before config
load when the process starts as a binary-upgrade child. This lets an upgraded
binary recover from a parent that was already polluted by older non-CLOEXEC
fds instead of preserving that fd garbage forever.

#### Local fd exhaustion no longer enters Patroni-assisted fallback

Backend connection failures caused by pg_doorman's own `EMFILE`/`ENFILE`
state are now classified as local resource exhaustion, not as PostgreSQL
unreachability. Those errors no longer blacklist the local backend or enter
the Patroni-assisted fallback discovery path, so fd pressure does not amplify
itself with fallback connection attempts and noisy discovery failures.

#### Web admin sockets use the safe TCP policy

Accepted Web UI and `/metrics` TCP sockets now receive the same low-risk TCP
keepalive, buffer-size and user-timeout configuration as other TCP sockets,
but do not inherit the pooler client `SO_LINGER` policy. This avoids abortive
HTTP closes when `general.tcp_so_linger = 0` while still bounding web socket
resource usage.

### 3.10.5

#### Binary upgrade survives a tight `RLIMIT_NOFILE`

SIGUSR2 binary upgrade now handles `EMFILE`/`ENFILE` from the old
process without spinning in the accept loop or overfilling the migration
queue.

* The TCP and Unix accept loops treat `EMFILE`/`ENFILE` as local resource
  pressure: they sleep for 10 ms and log at most once every 5 seconds.
  Other accept errors still log normally.

* The migration channel is no longer fixed at 4096 entries. At upgrade
  time pg_doorman reads the current `RLIMIT_NOFILE`, counts open fds via
  `/proc/self/fd`, reserves headroom for the handoff pipe/socketpair and
  per-client fd work, and caps the queue by the remaining budget. If no
  safe headroom remains, pg_doorman starts the new process without client
  migration and logs the budget decision.

* Client migration reserves a channel slot before calling `dup()` on the
  client fd. A full channel now applies backpressure before creating an
  extra fd.

If the pre-flight `pg_doorman -t` spawn fails with local `EMFILE`/`ENFILE`,
pg_doorman skips that validation step and continues with the binary
upgrade. Other validation failures still abort the upgrade before shutdown.

#### `/metrics` scrape uses cached socket-state counts

`/metrics` no longer walks `/proc/PID/net/tcp` and `/proc/PID/net/unix`
on the request path. On hosts with thousands of sockets, that synchronous
walk could hold worker threads long enough for regular Prometheus scrapes
to increase client p99.

Socket-state counts now live in a cached `ArcSwap` snapshot refreshed by a
background `spawn_blocking` task. The `/metrics` handler, periodic
`print_all_stats` output, and admin `SHOW SOCKETS` command read the cached
snapshot. The Web UI sockets endpoint still refreshes socket details on
demand for operator use.

The cache keeps scrape cost independent of the number of live sockets in
the common Prometheus path.

### 3.10.1

#### Configurable kernel TCP socket buffer size

New `general.tcp_socket_buffer_size` (`ByteSize`, default `0`). When set
to a non-zero value, pg_doorman calls `setsockopt(SO_RCVBUF/SO_SNDBUF)`
on every accepted client TCP socket and outbound backend TCP socket,
sets fixed send/receive buffer limits, and disables Linux TCP autotuning
for that socket. Linux applies/reports doubled values and may clamp them
by `net.core.rmem_max` / `net.core.wmem_max`.

The default `0` keeps the current behaviour (autotuning on). Operators
who observe `MemFree` jumping back up after a `pg_doorman` restart with
many long-lived idle clients may be seeing kernel TCP buffer
accumulation. This memory is not process RSS; depending on kernel and
cgroup mode it may show up as socket memory, for example `sock` in
cgroup v2 `memory.stat`. Those deployments can bound per-socket kernel
buffer limits by setting this knob to a value in the `64 KiB – 256 KiB`
range suitable for OLTP traffic in one datacenter. See the
[`tcp_socket_buffer_size`](reference/general.md#tcp_socket_buffer_size)
reference for details and trade-offs.

Config reloads do not resize already-open sockets. During `SIGUSR2`
binary upgrade, migrated client sockets are reconfigured in the new
process; backend sockets pick up the value only when opened or
reconnected.

Equivalent of PgBouncer's `tcp_socket_buffer` parameter. Odyssey and
PgCat have no analogue.

### 3.10.0

#### Prepared statements and startup-time planner parameters

`sync_server_parameters` now replays safe parameters sent by the client
in `StartupMessage`, not only the small set of PostgreSQL-reported
`ParameterStatus` values. This lets transaction-mode pools preserve
startup-time session state such as `search_path`,
`default_transaction_isolation`, and `role` when a client transaction
lands on a different backend connection. Configured
`startup_parameters` still win over client-supplied values.

The prepared-statement cache key now includes a digest of the
startup-time planner parameters that pg_doorman can safely replay:
`search_path`, `default_transaction_isolation`,
`default_transaction_read_only`, `default_text_search_config`, and
`role`. Two clients that prepare the same query under different
`search_path` values now get separate server-side prepared statements
instead of sharing one PostgreSQL plan.

Runtime `SET` for planner parameters that PostgreSQL does not report is
still not tracked. Clients that need to change those values after
connection startup should set them in `StartupMessage`, reconnect or
run `DISCARD ALL` after changing them, or disable `prepared_statements`
for that pool.

PgDoorman also rolls back optimistic per-backend prepared-statement LRU
entries when PostgreSQL rejects `Parse`. Reusing the same client
statement name after a failed Parse now forces a fresh Parse instead of
hitting a stale `DOORMAN_<N>` entry and surfacing SQLSTATE `26000`.

Per-pool response cache for `general.pooler_check_query`. The first
matching SimpleQuery in each pool's lifetime is forwarded to PostgreSQL;
every subsequent matching probe is answered from the cache without
touching the backend.

#### Behavior change for cold pools

Before this release pg_doorman answered any `pooler_check_query` match
locally with a hardcoded empty result. The default `;` came back instantly
without ever talking to PostgreSQL, and a non-empty value such as `select 1`
returned an empty response that did not match what a real PostgreSQL would
have produced.

The first probe per pool now does one PostgreSQL round-trip and captures
the real response. If PostgreSQL is unreachable at that moment, the
probing client sees a probe failure instead of an unconditional OK; the
earlier hardcode reported the pooler as healthy even when PostgreSQL was
down. Typical JDBC keepalive queries such as `select 1` (WildFly, HikariCP)
and `select 'pg_doorman'` now return the expected row.

#### Cache lifecycle

The cache is per pool and keyed by the query string. A `RELOAD` that
changes `pooler_check_query` invalidates the cache on the next ping; the
new value triggers one fresh backend probe and is then served from cache
until the value changes again. A reload that keeps the same value keeps
the cached response. `ErrorResponse` from the backend is forwarded to
the client unchanged and is never cached, so the next probe retries
against PostgreSQL.

#### Operator contract

`pooler_check_query` must be stable: the same input must produce the
same bytes, with no side effects. Safe values: `;`, `select 1`,
`select 'pg_doorman'`, `select version()`.

Unsafe values that the cache will silently freeze:

- `select now()`, `select clock_timestamp()` — the cached timestamp
  never advances.
- `select pg_is_in_recovery()` — a failover flips the role on
  PostgreSQL but the cached response still reports the old role.
- `select count(*) from <table>` — the cached count is whatever the
  first probe observed.
- `UPDATE`, `INSERT`, `DELETE`, `CALL`, `DO` — the side effect runs
  once and the success response is cached forever.

#### New metrics

- `pg_doorman_pooler_check_query_backend_total` — counter, increments
  on each probe forwarded to PostgreSQL (cache miss or
  RELOAD-induced re-probe).
- `pg_doorman_pooler_check_query_cache_total` — counter, increments
  on each probe served from the cache.

The ratio `cache_total / (cache_total + backend_total)` is the cache
hit rate.

#### Eviction visibility for prepared-statement caches

Per-eviction events from the named and anonymous query interner and
from the per-client anonymous LRU are now emitted as `TRACE` log
lines. The default `INFO` level is unchanged; turn them on at
runtime with

```
SET log_level = 'info,pg_doorman::server::prepared_statement_cache=trace,pg_doorman::client::protocol=trace';
```

The GC sweep task additionally emits one `DEBUG` aggregate line per
cycle that actually evicted something. Operators that previously had
only the aggregate `pg_doorman_query_interner_evictions_total` and
`pg_doorman_clients_prepared_anonymous_evictions_total` Prometheus
counters can now follow individual evictions during an incident.

The 80-char-with-ellipsis and 120-char preview helpers used in those
log lines live in a new `utils::strings` module and replace three
inline copies that had drifted apart.

#### Web UI lifecycle events

The sidebar used to toast "pg_doorman restarted — rate baseline reset"
on every routine RELOAD. Totals are summed across the live pool set,
and RELOAD plus dynamic-pool GC drop pools from that set, so the sum
legitimately falls without the process going anywhere. The heuristic
is gone. A real restart is detected by a change in `pid`,
`started_at_ms`, or `uptime_seconds`.

`/api/events` grows two new event targets:

- `PROCESS_START` — emitted once when setup finishes; carries the
  binary version and pid.
- `CONFIG_VALIDATION_ERROR` — emitted when SIGHUP, admin RELOAD, or
  `/api/admin/reload` rejects the new config. Rate-limited to one
  per second per target so a SIGHUP loop with a bad config cannot
  fill the 1024-entry ring with duplicates.

A persistent banner across the top of the UI replaces the transient
toast for conditions an operator must not miss:

- `shutdown_in_progress` — pg_doorman is draining.
- `migration_in_progress` — binary upgrade in flight.
- Last unresolved `CONFIG_VALIDATION_ERROR` — stays up until a
  successful `RELOAD` clears it.
- `/api/overview` silent for >15 s — banner switches to
  "pg_doorman unreachable — last contact 23s ago", so the operator
  knows the rest of the page is no longer trustworthy.

A no-op SIGHUP (config file re-parsed identically) now emits a
`RELOAD` entry with message `config unchanged` instead of going
silent — one event per signal keeps the audit timeline complete.

`/api/events` and `/api/overview` send `Cache-Control: no-store` so
intermediate proxies cannot collapse two consecutive polls into the
same response.

### 3.9.1

Web admin console refresh and a follow-up pass on `startup_parameters`.

Upgrade notes for operators monitoring 3.9.0:

- The pg_doorman-side budget rejection now returns `SQLSTATE 53400`
  (`configuration_limit_exceeded`) instead of `54000`. Alert rules
  and log filters keyed on `54000` need to switch.
- `PgDoormanStartupParameterPgRejection` is now `severity: warning`
  (was `critical` in 3.9.0). Cascade-overflow stays `critical`. Review
  the Alertmanager / on-call routing if you key on severity to page.

#### Web admin console

- Light theme by default. Three-position theme toggle (Light / System / Dark)
  in the sidebar footer; choice persists in localStorage.
- New `/servers` page reads SHOW SERVERS. Filters (database, user, state,
  application_name) and pagination live in the URL.
- New "Top SQLSTATE codes" card on Overview aggregating
  `errors_by_sqlstate` across pools.
- Patroni-assisted fallback banner on Overview when any pool reports
  `fallback_active=true`.
- Global RELOAD button on Config with typed confirmation.
- Logs and Clients filters move to URL parameters; deep links are
  shareable.
- Cmd+K / Ctrl-K command palette for navigation and pool lookup.
- `?` opens a keyboard-shortcut sheet. Esc dismisses popovers and
  leaves the war room.
- `/wall` requests a screen wake lock so a TV stays on past the OS
  screensaver timeout.
- Structured (i) popovers everywhere — definition, admin SHOW source,
  formula, thresholds, related metrics, link to docs.
- Sonner toast notifications for admin actions.
- Persistent transport indicator (http/https) in the sidebar footer.
- Counter-reset detection: a pg_doorman restart no longer renders as
  silent "0 qps" in the sidebar.
- Storage keys gained a host suffix, so two tabs against different
  poolers keep separate rolling buffers.
- Clients table memoises rows; poll cadence relaxed to 3 s. Resolves
  a memory growth reported on long sessions.
- Sidebar collapses below `md` (mobile navigation via Cmd+K and URL).
- Trimmed embedded font bundle: 5 woff2 (~146 KB) down from 9.

Backend: `web/access_log.rs` demotes authenticated 2xx reads to debug.
`info` covers admin actions, personal-data paths, `/api/auth/`,
`/api/sso/`, and any non-2xx.

Docs: `guides/web-ui.md` rewritten for the new pages and shortcuts.

#### startup_parameters follow-up

- If the resolved `startup_parameters` set exceeds the startup packet
  budget, backend startup now fails with `SQLSTATE 53400`. A
  deterministic `general + pool` overflow is rejected at config load.
- The final `ParameterStatus` messages sent to the client no longer
  overwrite operator-managed GUC names, so the client-visible values
  match the backend checkout state.
- `auth_query` now rebuilds a dynamic pool after a successful MD5
  refetch, rejects the stale-overlay race in `create_dynamic_pool`, and
  accepts native `json`/`jsonb` startup_parameter columns without a
  `::text` cast.
- `/api/config` and `/api/pools` show literal startup_parameter values
  only to `Admin`; SSO readers get the masked view. `/api/config` also
  marks `general.host`, `general.port`, `web.host`, and `web.port` as
  restart-required.
- Prometheus rules now cover PostgreSQL-side rejection, budget overflow,
  malformed auth_query columns, dedicated-mode drops, and rejected SSO
  credentials sent over insecure transport.
- Each pool now precomputes the merged startup map, budget decision, and
  canonical operator-key set. Backend checkout reuses those cached
  values instead of cloning and recalculating the map each time.

### 3.9.0

Per-pool PostgreSQL startup parameters. pg_doorman can now add
configured GUCs to each backend `StartupMessage`. Values apply in
three layers: `general.startup_parameters`, `pools.<name>.startup_parameters`,
and the optional `startup_parameters` column returned by passthrough
`auth_query`.

PostgreSQL stores these values as the session reset defaults, so
client-side `RESET ALL` and `DISCARD ALL` return to the configured value.
This gives one pool a different `plan_cache_mode`, `statement_timeout`,
`work_mem`, or `idle_in_transaction_session_timeout` without changing
`postgresql.conf`, `ALTER ROLE`, or `ALTER DATABASE`.

#### Cascade resolution

- `general.startup_parameters`, `pools.<name>.startup_parameters`, and
  the optional `startup_parameters` text column on an `auth_query` row
  are applied in order. Later layers override earlier ones per key.
- Dedicated `auth_query` mode uses a shared `server_user`, so
  pg_doorman ignores the per-user column there and logs one warning per
  pool and username.
- A reload that changes startup parameters recycles the affected pools.
  Idle backends with the old reset defaults are not reused.

#### Validation and protocol safety

- Reserved protocol keys (`user`, `database`, `replication`,
  `options`, the `_pq_.*` extension prefix) are refused at config load.
- Keys must match the PG GUC naming shape `[A-Za-z_][A-Za-z0-9_.]*`,
  values must not contain null bytes, and each level fits the startup-parameter
  budget of `MAX_STARTUP_PACKET_LENGTH - 512` bytes.
- The resolved parameter set is checked before each backend startup
  against PG's 10 000-byte `MAX_STARTUP_PACKET_LENGTH`. If only the
  auth_query layer overflows the packet, pg_doorman drops that layer and
  keeps the general/pool baseline. If the baseline itself does not fit,
  pg_doorman skips all configured keys for that spawn and logs the
  byte counts.

#### Behaviour on PG-side rejection

- If PostgreSQL rejects a configured startup parameter at backend
  startup, pg_doorman returns PostgreSQL's `ErrorResponse` to the
  client unchanged. pg_doorman does not retry without the key and does
  not disable the key automatically for the pool. Fix the parameter in
  the config; until then, backend startup for that pool fails with
  PostgreSQL's own SQLSTATE and message.
- SQLSTATEs with the `57P` prefix (server unavailable) keep mapping to
  `ServerUnavailableError` first so the Patroni-assisted fallback
  path can route around the failed node before the startup-parameter
  log line fires.
- The configured parameter wins over the client sync path:
  even if the client connect string carries an `application_name`
  (or another tracked GUC like `TimeZone`), the per-checkout
  `sync_parameters` call no longer overrides the configured value on
  the backend. That default stands until an
  explicit `SET` statement on the client session changes it.

#### RELOAD coherence

- A SIGHUP that changes `general.startup_parameters` drains pools that
  inherit that baseline. The per-pool config hash includes the general
  startup map, and carried-over dynamic `auth_query` pools are recycled
  when the baseline changes.

#### Observability

- `pg_doorman_backend_startup_parameter_errors_total{pool, sqlstate}`
  counts backend startups PostgreSQL rejected because of an
  configured startup parameter. The failing parameter name and username are
  written to the warning log line, not to metric labels.
- `SHOW STARTUP_PARAMETERS` (admin SQL console) lists the per-pool
  resolved parameters with the source of each value. `psql` tab
  completion on `SHOW <TAB>` now includes the command.
- The Web UI pool detail page shows the same rows in a "Startup
  parameters (configured)" section, driven by the new
  `startup_parameters[]` field on `/api/pools`.

See [PostgreSQL startup parameters](tutorials/startup-parameters.md)
for the configuration walkthrough, plus [General Settings](reference/general.md)
and [Pool Settings](reference/pool.md) for the full parameter list.

### 3.8.5

The web console now accepts JWTs issued by an external SSO proxy
alongside the existing Basic auth. The listener resolves every
request to one of three roles — `Anonymous`, `Sso` (read-only,
including logs and SQL text), and `Admin` (full access, including
`POST /api/admin/*`) — and a JWT can reach the `Admin` role through a
configurable group claim, so SSO operators run mutating admin actions
without sharing the Basic password. A per-request access log on a
dedicated logger target makes role transitions and 401/403 spikes
visible from `journalctl`. Full reference and an oauth2-proxy example
live in [`guides/web-ui.md`](guides/web-ui.md).

#### SSO authentication

- New `[web]` fields wire the SSO branch: `sso_enabled`,
  `sso_proxy_url`, `sso_public_key_file`, `sso_audience`,
  `sso_allowed_users`, `sso_groups_claim`, `sso_admin_groups`. JWTs
  are validated as RS256 against the PEM-encoded public key; the
  parsed key reloads on `RELOAD`.
- A JWT whose `sso_groups_claim` value intersects `sso_admin_groups`
  resolves to `Admin` with `auth_source = sso`. Empty
  `sso_admin_groups` (the default) keeps every SSO login on the
  read-only `Sso` role.
- Tokens are accepted from `Authorization: Bearer`, the
  `sso_access_token` cookie, or the `?token=` query parameter, in
  that priority order. Basic still wins over SSO when both are
  presented; a wrong Basic password no longer blocks a valid SSO
  token.
- `GET /api/auth/config` reports `sso_enabled`, `sso_proxy_url`,
  `sso_admin_groups_configured`, `sso_config_error`, and the resolved
  `current_user`, so the SPA renders the role-aware sign-in modal
  and sidebar without a second probe.

#### Role-aware gating

- `[web].ui_anonymous = false` now requires the `Sso` role for the
  public `/api/*` endpoints; previously every authenticated request
  needed `Admin`. Read-only privileged endpoints (`/api/logs`,
  `/api/prepared/text/*`, `/api/interner/top`, `/api/top/queries`)
  are reachable by `Sso` users. `POST /api/admin/*` remains
  `Admin`-only.
- Insufficient-role rejections return `403 Forbidden` with body
  `{"error":"forbidden","message":"admin role required"}`. Missing or
  invalid credentials still produce `401`. The SPA re-opens the
  sign-in modal on `401` and renders a non-blocking "admin role
  required" banner on `403`.

#### Browser sign-in flow

- The sign-in modal shows a **Sign in via SSO** button next to the
  Basic form when the backend reports `sso_proxy_url`. The proxy
  bounces the browser back with `?token=<jwt>`, which the SPA stores
  in `localStorage` and rewrites out of the URL.
- A silent-refresh poller (every 60 s, fires when `exp` is under
  90 s) opens a hidden iframe at `${origin}/?sso_silent=1`. The
  iframe renders a minimal `SilentCallback` and posts the new token
  to the parent. If silent refresh fails and a Basic credential is
  available, the SPA falls back to Basic without redirecting;
  otherwise it performs a full redirect through the SSO proxy.
- The SPA never sends cookies (`credentials: "omit"`); cookie auth
  remains available for curl, sidecars, and oauth2-proxy variants
  that paste the token into a cookie on the shared domain.

#### Access log

- Every response (200/401/403/404/5xx, `/metrics` scrapes included)
  emits one logfmt line on the dedicated `pg_doorman::web::access`
  target with `method`, `path`, `query` (presence flag only —
  raw query strings are never logged), `status`, `bytes`,
  `latency_ms`, `peer`, `auth_role`, `auth_source`, and `auth_user`.
  Bodies are not logged.
- Levels are picked per request. Admin actions, personal-data reads,
  every non-2xx response, and any authenticated request log at
  `info`. Anonymous successful reads of public APIs and `/metrics`
  scrapes log at `debug`, so `RUST_LOG=info` no longer drowns in
  scrape noise.

#### Real client IP behind a reverse proxy

- New `[web].trusted_proxies` CIDR list. When the TCP peer falls in
  this list, the access log parses `X-Forwarded-For` (or RFC 7239
  `Forwarded`), walks the chain right-to-left skipping further
  trusted hops, and uses the first untrusted address as `peer`. An
  untrusted client that sends `X-Forwarded-For` is ignored, so the
  field cannot be spoofed.

#### Observability

- New gauges `pg_doorman_web_sso_enabled` and
  `pg_doorman_web_sso_config_error`. The latter stays at `1` while
  `sso_enabled = true` but the runtime failed to load (missing PEM
  file, empty audience, unparsable PEM). The exact reason is
  exported through `/api/auth/config.sso_config_error` and rendered
  as a banner in the SPA.
- New counters `pg_doorman_web_auth_attempts_total{role,source}`,
  `pg_doorman_web_requests_total{status_class,role}`, and
  `pg_doorman_web_sso_validation_errors_total{reason}` (reasons:
  `signature`, `expired`, `audience`, `no_username`, `allowlist`).
  Operators alert on SSO degradation without grepping logs.

### 3.8.0

#### Added

**Built-in operator dashboard.** pg_doorman exposes a single-page
diagnostic console on the same port as `/metrics`, served from
inside the binary and gated on `[web].ui = true` plus a non-default
`admin_password`. Getting comparable detail from the existing psql
admin console means running `SHOW POOLS`, `SHOW CLIENTS`,
`SHOW STATS` and friends in a loop, computing rates by hand between
two snapshots, and joining the rows mentally. The dashboard does
that on a 1.5 s tick.

What it shows that the psql admin console does not:

- **Live time-series, not snapshots.** Latency p95/p99, qps,
  errors/s and connection saturation render as sparklines, so
  "spiking now" is visually distinct from "always been like this".
- **Errors broken down by SQLSTATE per pool.** Plus top-N stuck
  queries by `current_query_age_ms`, top-N noisy clients by
  errors, top-N hottest prepared statements by hit rate.
- **Process memory by category.** RSS split into jemalloc live
  allocations, jemalloc fragmentation, internal pg_doorman caches,
  code + libs, stacks + page tables, swap and anonymous remainder,
  with cgroup current / max alongside. Every category carries a
  one-line explanation on hover.
- **Per-thread tokio-worker CPU.** Drill-down from the threads
  count to per-thread utilisation, so a stuck worker is visible
  without `perf top` on the host.
- **Live log tail.** An in-process LogTap activates on the first
  `/api/logs` request and self-disables two minutes after the last
  viewer. Level and target filters apply client-side over the
  rolling buffer.
- **Sortable, filterable tables.** Pools, Clients, Apps and Caches
  sort by any column and filter by substring; Prepared statements
  adds a kind dropdown on top.

The dashboard is read-only by default. Pause / Resume / Reconnect /
Reload are the four writes, scoped to one pool via
`?pool=user@db`, to every pool of a database via `?db=`, or
globally — the same semantics as the admin protocol.

#### Notes

- `[web].ui_anonymous` (default `false`) controls whether the
  read-only `/api/*` endpoints answer without basic auth. Admin-
  only endpoints (`/api/logs`, `/api/admin/*`,
  `/api/prepared/text/{hash}`, `/api/interner/top`,
  `/api/top/queries`) always require it regardless of that flag.
- The dashboard polls every 1.5 s, but a 250 ms shared snapshot
  feeds `/api/overview`, `/api/pools`, `/api/clients`,
  `/api/servers`, `/api/apps`, `/api/stats` and `/metrics`, so a
  multi-tab dashboard does not multiply pool-stats work by the
  number of open tabs.

### 3.7.0

#### ACTION REQUIRED before upgrading to 3.7.0

- **SQLSTATE for missing prepared statements changed from `58000` to
  `26000`.** Any `Bind` or `Describe` referencing a prepared statement
  that pg_doorman cannot resolve now returns SQLSTATE `26000`
  (`invalid_sql_statement_name`), matching native PostgreSQL.
  Audit dashboards, log searches, alert rules, and retry middleware
  that filter on `58000` for this condition (Splunk saved searches,
  Grafana log alerts, custom retry policies). Drivers that auto-retry
  on `26000` (pgjdbc, pgx with `cache_describe`) now do so;
  drivers that closed the connection on `58000` will no longer.
- **Migration format v1 is no longer accepted.** Upgrades from a
  pg_doorman that emitted v1 (3.5.0–3.5.x) must hop through
  3.6.x first; from 3.4 and earlier no migration support existed,
  so the upgrade is unaffected.
- **`client_prepared_statements_cache_size` is deprecated.** It
  remains a serde alias of `client_anonymous_prepared_cache_size`,
  with a `WARN` at startup. Planned for removal in 3.9; rename in
  configs now.
- **Anonymous prepared statements have a TTL by default.** The
  query interner evicts an anonymous entry after
  `query_interner_anon_idle_ttl_seconds` (default 60) of idle time.
  Drivers like pgjdbc and `pgx` with `cache_describe` re-issue
  `Parse` transparently when the next `Bind` returns SQLSTATE
  `26000`. If your driver relies on cross-batch unnamed prepared
  statements without a re-Parse, set
  `query_interner_anon_idle_ttl_seconds: 0` to keep the pre-3.7
  unbounded behaviour.

#### Added

- The query interner is split into NAMED (passive `Arc::strong_count`
  GC) and ANON (idle TTL). Two general knobs control the GC:
  `query_interner_gc_interval_seconds` (default 60, restart-only) and
  `query_interner_anon_idle_ttl_seconds` (default 60; `0` disables
  TTL and restores pre-3.7 unbounded behaviour; live-reloadable).
  A two-cycle mark-and-sweep grace prevents eviction of entries
  touched between cycles.
- `SHOW INTERNER` reports entries and bytes per kind;
  `SHOW INTERNER N` lists the top N by interned text length with
  hash, kind, idle_ms, and a 120-character preview; `RESET INTERNER`
  clears both halves (diagnostics-only).
- Prometheus interner metrics:
  `pg_doorman_query_interner_entries{kind}`,
  `_bytes{kind}`, `_evictions_total{kind, reason}`,
  `_synthetic_misses_total`, `_gc_duration_seconds`.
- `server_prepared_statements_cache_size` (general + per-pool)
  sizes the per-backend server-level prepared-statement LRU.
  When unset, inherits `prepared_statements_cache_size`.
- `client_anonymous_prepared_cache_size` bounds the Anonymous part
  of the per-client cache; named statements remain unbounded. The
  knob is now optional and inherits `prepared_statements_cache_size`
  when unset (`0` still means unlimited).
- `kind` column appended to `SHOW PREPARED_STATEMENTS`
  (`named` / `anonymous` / `mixed`).
- `SHOW POOLS_MEMORY` gains `client_named_count`,
  `client_anonymous_count`, and `client_anonymous_evictions_alive`
  (a gauge of evictions across currently connected clients; the
  authoritative cumulative counter lives in Prometheus as
  `pg_doorman_clients_prepared_anonymous_evictions_total`). The
  matching gauges `pg_doorman_clients_prepared_named_entries` /
  `..._anonymous_entries` round out the surface.

#### Changed

- The per-client prepared-statement cache is split into Named
  (unbounded) and Anonymous (LRU). Fixes a bug where the previous
  combined LRU could evict a Named entry and cause the next `Bind`
  to fail with `prepared statement does not exist`.
- `Bind` against an anonymous prepared statement that is no longer
  cached anywhere (interner, pool, client) now returns SQLSTATE
  `26000` (`invalid_sql_statement_name`) instead of `58000`,
  matching native PostgreSQL. Standard drivers re-issue `Parse`
  transparently.

#### Deprecated

- `client_prepared_statements_cache_size` is renamed to
  `client_anonymous_prepared_cache_size`. The old name remains a
  serde alias and logs a `WARN` at startup; rename it in your
  config.

#### Removed

- Migration format v1 is no longer accepted. Upgrades from versions
  that emitted v1 (3.4 and earlier) must hop through a 3.5–3.6
  binary first; `deserialize_state` returns `unsupported version 1`
  otherwise.

### 3.6.5 <small>May 4, 2026</small>

#### Fix: stuck `cl_active/sv_active` after large DataRow client disconnect under pressure

When a large `DataRow` was deferred via `pending_large_message`, `recv()` cleared the deferred header before streaming. If the client disconnected during streaming write, the next drain/read path lost frame boundaries and could block in `wait_available()`. Under full pressure, this left `cl_active`/`sv_active` pinned at pool size and prevented normal `server_lifetime` recycling.

`recv()` now keeps `pending_large_message` until large-message handling succeeds and clears it only on `Ok`. On error, the next `recv()` still has correct frame context, allowing cleanup to complete and active counters to drop as expected.

#### Observability: `oldest_active_age_ms` per pool

`SHOW POOLS` exposes a new `oldest_active_age_ms` column and Prometheus exports `pg_doorman_pools_oldest_active_age_ms{user, database}`. The gauge reports the maximum age in milliseconds among ACTIVE servers in each pool, taken at snapshot time, and falls back to `0` when no server is currently ACTIVE. Sustained non-zero values flag stuck checkouts before pool exhaustion.

### 3.6.4 <small>Apr 29, 2026</small>

#### Fallback resilience

Patroni-assisted fallback now races `Server::startup` against every alive cluster member in parallel, with a strict sync_standby priority that protects write traffic during a local-backend outage. See [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md) for operator-level details.

- **Startup deadline per candidate.** `Server::startup` runs under `tokio::time::timeout`. Main path: `connect_timeout` (default `3s`), now also covers the StartupMessage round-trip. Fallback path: `fallback_connect_timeout` (default `5s`) per candidate. Raise `connect_timeout` if local startup legitimately exceeds 3s (large WAL replay after restart).
- **Two-wave parallel race.** Wave 1 races startup against every `sync_standby` in parallel and takes the first success; wave 2 (replica + leader) runs only if every sync_standby failed or none existed. While any sync_standby is still in-flight, a replica that already finished startup is intentionally not used — the user-facing requirement is "sync wins if it's alive at all", because the sync_standby is the lowest-data-loss promotion target. On full exhaustion the doorman log records `all fallback candidates rejected (3 startup_error, 1 timeout)` with a deterministic per-reason breakdown; the client always sees the sanitized `Unable to retrieve server parameters … may be unavailable or misconfigured` FATAL — read the doorman log for the wave/winner trace.
- **Per-host cooldown with exponential backoff.** Failed candidate is marked unhealthy for `fallback_connect_timeout`, doubling on consecutive failures up to `60s`; resets to base after the window elapses. The cooldown map is pruned of expired entries at the start of each discovery cycle, so its size stays linear in actively-failing candidates rather than accumulating dead pod IPs.
- **Soft outer deadline.** The full fallback path runs under `query_wait_timeout` (default `5s`). If it fires, pg_doorman aborts cleanly with `fallback: outer deadline {ms}ms exceeded` in the log and returns the sanitized FATAL to the client. Per-candidate timeouts are the hard guarantee against hangs; the outer deadline is a soft cap on how long the client itself is willing to wait.
- **Whitelist post-failure rediscovery.** Stale cached host failure clears the cache and runs one extra discovery round.
- **Log rate-limit.** Per-candidate `WARN` rate-limited to 1 per 10s per `(pool, host:port)`; suppressed lines log at DEBUG.
- **`pg_doorman_fallback_host` cleanup on switchover.** Old `(host, port)` label removed when whitelist changes.
- **New metric** `pg_doorman_fallback_candidate_failures_total{pool, reason}`. Reasons: `connect_error`, `startup_error`, `server_unavailable`, `timeout`, `other`.

Use IP addresses (not hostnames) in `member.host`: a 5s DNS hang consumes the full per-candidate budget.

### 3.6.3 <small>Apr 28, 2026</small>

#### Fix: per-connection read buffer leak under multi-MiB simple-query INSERTs

Per-connection reusable read buffers (`Client.read_buf`, `Server.read_buf`) retained the largest allocation each connection had served. After one multi-MiB simple-query INSERT, every subsequent small message split out of that allocation, and the reusable buffer reclaimed the multi-MiB region as soon as the previous `BytesMut` was dropped. Across thousands of clients in transaction mode, occasional megabyte-sized payloads compounded into a 100 MB → 4 GB pooler RSS regression.

`read_message_reuse` and `read_message_body_reuse` now drop the backing allocation before each read when the buffer's capacity exceeds 256 KiB and fall back to a fresh 16 KiB buffer. The steady-state path (capacity within threshold) is unchanged.

### 3.6.2 <small>Apr 27, 2026</small>

**New features:**

- **Unix socket listener.** `unix_socket_dir` creates `.s.PGSQL.<port>` socket file. Connect with `psql -h <dir>` or `pgbench -h <dir>`. No TCP overhead on local connections.

- **HBA `local` rule matching.** `local` rules in pg_hba now apply to Unix socket connections. `host`/`hostssl`/`hostnossl` rules apply only to TCP. Previously `local` rules were parsed but ignored.

- **`unix_socket_mode` controls socket file permissions.** New `[general]` setting fixes the permission bits on `.s.PGSQL.<port>` after bind, so the access surface no longer depends on the process umask. Octal string, default `"0600"` (owner only). Set to `"0660"` to grant a Unix group, or `"0666"` to allow any local user. Validated at config load — invalid octal values, setuid/setgid/sticky bits, and overflow into bits above `0o777` are rejected upfront.

**Known limitations (Unix socket):**

- Unix listener not handed off during `SIGUSR2` binary upgrade. New process re-creates the socket; connections refused for ~100ms.
- `only_ssl_connections` does not reject Unix socket connections. Unix sockets do not need TLS for transport security.

### 3.6.1 <small>Apr 27, 2026</small>

#### openssl 0.10.78 (CVE-2026-41678, CVE-2026-41681)

`openssl` 0.10.72 is affected by CVE-2026-41678 and CVE-2026-41681; some registry mirrors refuse downloads on that basis. pg_doorman now depends on `openssl` 0.10.78 and `openssl-sys` 0.9.114. API-compatible — no source changes.

### 3.6.0 <small>Apr 24, 2026</small>

#### Patroni-assisted fallback

When pg_doorman runs next to PostgreSQL on the same machine and connects via unix socket, a Patroni switchover or PostgreSQL crash leaves the pooler without a backend. With `patroni_api_urls` configured, pg_doorman queries the Patroni REST API `/cluster` endpoint, picks a live cluster member, and routes new connections there.

Candidate selection: `sync_standby` first (most likely next leader), then `replica`, then any other member. Members with `noloadbalance`, `nofailover`, or `archive` tags are excluded. All candidates are TCP-probed in parallel; the first responding `sync_standby` wins immediately.

The local backend stays in cooldown for `fallback_cooldown` (default 30s). During the cooldown, subsequent connection requests reuse the cached fallback host without re-querying Patroni. Fallback connections use a short `fallback_lifetime` (defaults to `fallback_cooldown`) so the pool returns to the local backend once it recovers.

Configuration:

```yaml
pools:
  mydb:
    patroni_api_urls:
      - "http://10.0.0.1:8008"
      - "http://10.0.0.2:8008"
    fallback_cooldown: "30s"
    patroni_api_timeout: "5s"
    fallback_connect_timeout: "5s"
```

Prometheus metrics: `pg_doorman_patroni_api_requests_total`, `pg_doorman_fallback_connections_total`, `pg_doorman_patroni_api_errors_total`, `pg_doorman_fallback_active`, `pg_doorman_patroni_api_duration_seconds`, `pg_doorman_fallback_host`, `pg_doorman_fallback_cache_hits_total`.

If you tracked this feature under its working name in 3.5.x dev builds, the config keys and metric names changed before the public release: `patroni_discovery_urls` → `patroni_api_urls`, `failover_blacklist_duration` → `fallback_cooldown`, `failover_discovery_timeout` → `patroni_api_timeout`, `failover_connect_timeout` → `fallback_connect_timeout`, `failover_server_lifetime` → `fallback_lifetime`. Old `pg_doorman_failover_*` metrics are renamed to `pg_doorman_patroni_api_*` / `pg_doorman_fallback_*`.

#### Server-side TLS (pg_doorman → PostgreSQL)

Six SSL modes matching libpq semantics: `disable`, `allow` (default), `prefer`, `require`, `verify-ca`, `verify-full`. Mutual TLS supported via `server_tls_certificate` / `server_tls_private_key`.

Configuration is per-pool with global defaults in `[general]`. Cancel requests use TLS when the main connection used TLS.

**Breaking change:** `server_tls` (bool) and `verify_server_certificate` (bool) are removed. They were parsed but non-functional. Replace with:

| Old config | New config |
|-----------|-----------|
| `server_tls: false` | `server_tls_mode: "disable"` |
| `server_tls: true` | `server_tls_mode: "require"` |
| `server_tls: true` + `verify_server_certificate: true` | `server_tls_mode: "verify-full"` |
| (not set) | `server_tls_mode: "allow"` (new default) |

The new default `allow` tries plain TCP first. If the server rejects the connection (e.g. `pg_hba.conf` requires TLS), pg_doorman retries with TLS on a new TCP socket. This matches libpq `sslmode=allow`.

`SHOW SERVERS` now includes a `tls` column showing whether each backend connection uses TLS.

### 3.5.3 <small>Apr 22, 2026</small>

#### Prepared statement cache overflow under concurrent load

The pool-level prepared statement cache could grow well above its configured `prepared_statements_cache_size` under concurrent client traffic. Production showed 480 entries with a limit of 300. The check-then-insert sequence in the cache had a race: multiple clients passed the size check simultaneously, each inserted without evicting. Now insertion happens first, followed by eviction in a loop until the cache is within bounds.

### 3.5.2 <small>Apr 21, 2026</small>

#### Semaphore permit leak on direct handoff

Each `return_object` handoff (delivering a connection to a waiting client via oneshot channel) permanently consumed one semaphore permit. After `max_size` handoffs the pool semaphore was fully drained, blocking all new `timeout_get` callers. The pool could not create connections and stabilized at whatever size it reached during cold start (typically 4-8 out of 40).

Root cause: `wrap_checkout` calls `permit.forget()`, and the handoff path in `return_object` skipped `add_permits(1)`. Now `return_object` restores the permit on both the handoff and idle-queue paths. Compensating `add_permits(1)` in `pre_replace_one` removed (no longer needed).

#### Burst gate select race

The `tokio::select!` in the burst gate loop randomly picked among ready branches. When `sleep(5ms)` or `create_done` won over an already-delivered oneshot, the connection was silently dropped, inflating `slots.size` without a live server. Fixed with `biased;` (oneshot checked first) and a `try_recv` drain that pushes orphaned connections to idle without double-counting the permit.

#### Migration fixes

- **Client ID collision after migration.** The new process started its connection counter at 0, colliding with migrated client IDs. Now the counter advances past the highest migrated ID.

- **SCRAM passthrough state preserved.** The ClientKey from the first client's SCRAM handshake is serialized in the migration payload (v2 format, backward compatible). The new process skips the `ScramPending` fallback to `server_password`.

#### Session mode statistics fix

`xact_time` percentiles in session mode showed the entire session duration instead of individual transaction time. Now recorded per-transaction at each `ReadyForQuery(Idle)`, matching transaction mode semantics.

`query_time` had the same accumulation bug: the timer was set once before the inner loop and never reset, so each subsequent query reported the cumulative session duration. Now reset per-query in session mode.

#### Adaptive anticipation budget

Anticipation wait (formerly fixed 300-500ms) scales with real transaction latency: `xact_p99 * 2 +/- 20%` jitter, clamped to [5ms, 500ms]. Cold start default: 100ms.

#### Diagnostic logging

Slow checkout warnings (>500ms) now include pool state: `size`, `avail`, `waiting`, `inflight`, `creates`, `gate_waits`, `antic_ok`, `antic_to`, `fallback`. Phase-specific warnings added for semaphore timeout, burst gate timeout, coordinator exhaustion, and create failure.

### 3.5.1 <small>Apr 20, 2026</small>

#### systemd Type=notify support

pg_doorman now sends `sd_notify(READY=1)` on startup and `sd_notify(MAINPID=<child_pid>)` during binary upgrade. With `Type=notify` in the systemd unit, `systemctl reload` performs a zero-downtime binary upgrade without PID tracking issues — systemd follows the new process correctly and does not restart the service.

The shipped `pg_doorman.service` changes from `Type=forking` + `--daemon` to `Type=notify` (foreground). Existing installations using `--daemon` continue to work but do not benefit from client migration.

Docker `STOPSIGNAL` changed from `SIGINT` to `SIGTERM` to prevent binary upgrade in containers (where PID 1 exit kills the container).

### 3.5.0 <small>Apr 15, 2026</small>

#### Client migration during binary upgrade

Idle clients now transfer to the new process via Unix socket (`SCM_RIGHTS`) without reconnecting. Active-transaction clients finish their transaction on the old process, then migrate. Prepared statement caches are serialized and transparently re-parsed on the new backend. The old process exits once all clients have migrated or `shutdown_timeout` expires.

#### TLS connection migration (opt-in)

Build with `--features tls-migration` to migrate TLS sessions without re-handshake. A patched vendored OpenSSL 3.5.5 exports/imports symmetric cipher state (keys, IVs, sequence numbers). Linux-only. Offline builds supported via `OPENSSL_SOURCE_TARBALL` env var with SHA-256 verification.

### 3.4.0 <small>Apr 11, 2026</small>

#### Pool Coordinator — database-level connection limits

When multiple user pools share one PostgreSQL database, the sum of their `pool_size` values can exceed `max_connections`. A spike in one pool starves the others, or PostgreSQL rejects connections outright.

`max_db_connections` caps total backend connections per database across all user pools. When the cap is reached, the coordinator frees capacity through three mechanisms, tried in order:

1. **Reserve pool.** If `reserve_pool_size > 0` and the reserve has headroom, a permit is granted immediately — no eviction, no wait. The reserve is a burst buffer: idle reserve connections are upgraded to main permits by the retain cycle once pressure drops, and closed if they stay idle longer than `min_connection_lifetime`.

2. **Eviction.** The coordinator closes one idle connection from a peer pool with the largest surplus above its `min_guaranteed_pool_size` floor. Candidates are ranked by p95 transaction time — slow pools donate first, because a 1 ms reconnect cost is negligible against a 15 ms p95 but doubles a 0.96 ms one. Only connections older than `min_connection_lifetime` (default 30 s) are eligible, which suppresses cyclic reconnect between pools that take turns stealing slots.

3. **Wait.** If nothing is evictable, the caller parks for up to `reserve_pool_timeout` (default 3 s), waking on any peer connection return or permit drop. After the wait, the reserve is retried once more before the client receives an error.

Disabled by default (`max_db_connections = 0`) — zero overhead when not configured. The hot path (idle connection reuse) never touches the coordinator; only new connection creation does, at the cost of one atomic operation.

New pool-level config fields:

| Parameter | Default | Purpose |
|---|---|---|
| `max_db_connections` | `0` (disabled) | Hard cap on backend connections per database |
| `min_connection_lifetime` | `30000` ms | Eviction age floor — connections younger than this are immune |
| `reserve_pool_size` | `0` (disabled) | Extra permits above the cap, granted on burst |
| `reserve_pool_timeout` | `3000` ms | Coordinator wait budget before error |
| `min_guaranteed_pool_size` | `0` | Per-user eviction protection floor |

New admin commands: `SHOW POOL_COORDINATOR` (per-database coordinator state), `SHOW POOL_SCALING` (per-pool checkout counters). Both are also exported as Prometheus metrics under `pg_doorman_pool_coordinator{type, database}` and `pg_doorman_pool_scaling{type, user, database}`.

See the [pool pressure tutorial](tutorials/pool-pressure.md) for acquisition phases, tuning recipes, and alert examples.

#### Connection checkout under pressure

Replaces `scaling_cooldown_sleep` (a fixed 10 ms delay before creating a backend connection) with a multi-phase checkout that reuses connections about to be returned before resorting to `connect()`.

When the idle pool is empty and the pool is above its warm threshold (`scaling_warm_pool_ratio`, default 20%), a caller first spins briefly (`scaling_fast_retries`, default 10 yield iterations), then registers a direct-handoff waiter. Connections returned by other clients are delivered through the waiter channel — no idle-queue round-trip, no race with other checkout attempts. The waiter deadline is bounded by `query_wait_timeout` minus a 500 ms reserve for the create path. If no connection arrives, the caller proceeds to create.

Backend `connect()` calls are capped at `scaling_max_parallel_creates` (default 2) per pool. Callers above the cap wait for a peer create to finish or a connection to be returned. Background replenish (`min_pool_size`) respects the same cap and defers to the next retain cycle when the gate is full, so it does not compete with client-driven creates during spikes.

Connections nearing `server_lifetime` expiry (95% of age) trigger a pre-replacement: a background task creates a successor before the old connection fails recycle, so the next checkout hits the hot path.

The direct-handoff queue is FIFO. On a 500-client / 40-connection AWS Fargate benchmark, p99/p50 ratio is 1.08 (pg_doorman) vs 25.5 (Odyssey). Every client pays roughly the same queue cost.

**Migration:** remove `scaling_cooldown_sleep` from your config if present. Replace with `scaling_max_parallel_creates` (default 2) if you need to tune the concurrency cap.

**Improvements:**

- **Runtime log level control.** `SET log_level = 'debug'` changes the log filter without restart; `SET log_level = 'warn,pg_doorman::pool::pool_coordinator=debug'` targets specific modules. `SHOW LOG_LEVEL` displays the current filter. Changes are ephemeral (lost on restart).

- **Log readability overhaul.** Consistent `[user@pool #cN]` prefix. Durations as `4m30s` instead of raw milliseconds. Stats line in logfmt. PG error newlines escaped. Expensive debug computations guarded by `log_enabled!()` to avoid allocations at production log levels.

- **Auth failure logs include client IP.** SCRAM, MD5, JWT, and PAM failures show the source address.

- **Replenish failure noise suppression.** Repeated `min_pool_size` failures log once at warn, then a periodic reminder every ~10 minutes with the failure count.

- **`avg_xact_time` column in `SHOW POOLS`.** Average transaction time per pool, visible alongside existing connection counts.

- **Smart session cleanup in transaction mode.** pg_doorman tracks which session state a client dirtied (`SET`, `DECLARE CURSOR`, prepared statements) and sends the matching reset on checkin. If the client cleaned up after itself — `RESET ALL`, `CLOSE ALL`, `DEALLOCATE ALL`, or `DISCARD ALL` — pg_doorman sees the confirmation and skips its own reset. Drivers like `jackc/pgx` that send a cleanup batch on disconnect no longer cause a redundant round-trip to PostgreSQL. A `SET` without a follow-up reset still triggers cleanup as before.

### 3.3.5 <small>Mar 31, 2026</small>

**Bug Fixes:**

- **Prepared statement eviction during batch breaks buffered Bind.** When a client sent a batch like `Parse(A), Bind(A), Parse(C), Sync` and `Parse(C)` triggered server-side LRU eviction of statement A, the `Close(A)` was sent to PostgreSQL immediately (out-of-band), deleting A before the client buffer was flushed. `Bind(A)` then failed with `prepared statement "DOORMAN_X" does not exist` (error 26000). Two fixes: (1) `has_prepared_statement()` now promotes entries in the LRU on access (`get()` instead of `contains()`), so actively-used statements resist eviction. (2) Eviction `Close` is deferred until after the batch completes — the statement stays alive on PostgreSQL while Binds in the buffer are processed, then `Close` is sent as post-batch cleanup. If the client disconnects before `Sync`, `checkin_cleanup` detects the pending deferred closes and triggers `DEALLOCATE ALL`.

### 3.3.4 <small>Mar 30, 2026</small>

**Bug Fixes:**

- **Prepared statement cache desync after client disconnect.** When a client sent Parse but disconnected before Sync/Flush, pg_doorman registered the statement in the server-side LRU cache but never sent the actual Parse to PostgreSQL (it was still in the client buffer, which was dropped on disconnect). The next client that got the same server connection and used the same query saw the stale cache entry, skipped sending Parse, and received `prepared statement "DOORMAN_X" does not exist` (error 26000) from PostgreSQL. Fixed by tracking a `has_pending_cache_entries` flag on the server connection: set when a statement is added to the cache without immediate Parse confirmation, cleared after successful buffer flush. If the client disconnects before flushing, `checkin_cleanup` detects the flag and triggers `DEALLOCATE ALL` to re-synchronize the cache. Zero overhead on the normal path (one boolean check per checkin).

### 3.3.3 <small>Mar 26, 2026</small>

**Bug Fixes:**

- **Log spam from missing `/proc/net/tcp6` when IPv6 disabled.** `get_socket_states_count` failed entirely if any of the three /proc files was absent, logging errors every 15 seconds and losing tcp/unix metrics that were available. Missing files are now skipped — counters stay at zero. Other I/O errors (permission denied) still propagate.

- **Protocol violation when streaming large DataRow with cached prepared statements.** `handle_large_data_row` wrote accumulated protocol messages (BindComplete, RowDescription) directly to the client socket, bypassing `reorder_parse_complete_responses`. When Parse was skipped (prepared statement cache hit), the client received BindComplete without the synthetic ParseComplete — causing `Received backend message BindComplete while expecting ParseCompleteMessage` in Npgsql and similar drivers. Triggered when `message_size_to_be_stream` ≤ 64KB. Fixed by returning accumulated messages from `recv()` before entering the streaming path, so response reordering runs first. Same fix applied to `handle_large_copy_data`.

### 3.3.2 <small>Mar 1, 2026</small>

**Breaking Changes:**

- **`auth_query` config field renames**: Two fields in the `auth_query` section have been renamed for clarity. `auth_query.pool_size` (number of connections for running auth queries) is now `auth_query.workers`. `auth_query.default_pool_size` (data pool size for dynamic users) is now `auth_query.pool_size`, matching the same parameter name used in static pools. **Migration**: rename `pool_size` to `workers` and `default_pool_size` to `pool_size` in your `auth_query` config. If you don't update, the old `pool_size` value (typically 1-2) will be interpreted as the data pool size, drastically reducing connection capacity. The old `default_pool_size` key is silently ignored and defaults to 40.

**Bug Fixes:**

- **Session mode: keep server connections alive after SQL errors.** A query like `SELECT 1/0` returns an `ErrorResponse` from PostgreSQL but leaves the connection fully usable. Previously, `handle_error_response` called `mark_bad()` unconditionally in async mode, so the connection was destroyed at session end. Now `mark_bad` is skipped when the pool runs in session mode. Transaction mode still calls `mark_bad` because the connection returns to a shared pool where protocol desync is dangerous.

- **Pool-level `server_lifetime` and `idle_timeout` overrides ignored**: Pool-level overrides for `server_lifetime` and `idle_timeout` were silently ignored — the general (global) values were always used instead. Fixed in 6 places across 3 pool creation contexts (static pools, auth_query shared pools, dynamic pools). Now `pool.server_lifetime` and `pool.idle_timeout` correctly override the general settings when specified.

- **`idle_timeout` default was 83 hours instead of 10 minutes**: The default `idle_timeout` was set to 300,000,000ms (83 hours), effectively disabling idle connection cleanup. Idle server connections could accumulate indefinitely. Changed default to 600,000ms (10 minutes).

- **`retain_connections_max` quota exhaustion causing unlimited closure**: When `retain_connections_max > 0` and the global counter reached the limit, the remaining quota became `0` via `saturating_sub`. Since `0` means "unlimited" in `retain_oldest_first()`, pools processed after quota exhaustion lost ALL idle connections in a single retain cycle instead of none. With non-deterministic HashMap iteration order, this bug manifested as random pools losing all connections. Fixed by adding an early return when the quota is exhausted.

- **`retain_connections_max` doc comment incorrectly stated default as `0` (unlimited)**: The actual default is `3`.

- **`server_lifetime` default changed from 5 minutes to 20 minutes**: The previous default of 5 minutes was shorter than `idle_timeout` (10 minutes), which meant `idle_timeout` could never trigger — connections were always killed by `server_lifetime` first. Changed to 20 minutes so that `idle_timeout` (10 min) handles idle cleanup while `server_lifetime` (20 min) rotates long-lived connections. Note: `idle_timeout` only applies to connections that have been used at least once — prewarmed/replenished connections that were never checked out by a client are not subject to `idle_timeout` and will only be closed when `server_lifetime` expires.

- **`idle_timeout = 0` did not disable idle timeout**: `idle_timeout = 0` should disable idle connection cleanup, matching PgBouncer's `server_idle_timeout = 0` and pg_doorman's `server_lifetime = 0`. Instead, pg_doorman closed connections after ~1 ms of idle time. Fixed by adding an `idle_timeout_ms > 0` guard before the elapsed-time check.

- **`idle_timeout` had no jitter — synchronized mass closures**: Unlike `server_lifetime` which applies ±20% per-connection jitter to prevent thundering herd, `idle_timeout` used a single pool-wide value. When many connections became idle simultaneously (e.g., after a traffic burst), they all expired at the exact same moment, causing mass closures in one retain cycle. Now `idle_timeout` applies the same ±20% per-connection jitter as `server_lifetime`.

- **`retain_connections_max` unfair quota distribution across pools**: The retain cycle iterated pools via HashMap, whose order is deterministic within a process (fixed RandomState seed). The same pool always got iterated first and consumed the entire `retain_connections_max` quota, starving other pools. Expired connections in starved pools were never cleaned up by retain — clients had to discover them via failed `recycle()` checks, adding latency. Fixed by shuffling pool iteration order each cycle.

- **Retain and replenish used separate pool snapshots**: The retain and replenish phases each called `get_all_pools()` separately. If `POOLS` was atomically updated between them (config reload, dynamic pool GC), retain operated on one set of pools and replenish on another, potentially missing pools that need replenishment. Fixed by using a single snapshot for both phases.

**Testing:**

- **PHP PDO_PGSQL driver added to test infrastructure.** PHP 8.4 with `pdo_pgsql` extension is now included in the Nix-based Docker test image. Two BDD scenarios verify basic connectivity (SELECT 1) and session mode behavior (SQL error does not change backend PID). Run with `make test-php` or `--tags @php`.

**New Features:**

- **`pool_size` observability**: New `pg_doorman_pool_size` Prometheus gauge exposes the configured maximum pool size per user/database. The `pool_size` column is also added to `SHOW POOLS` and `SHOW POOLS_EXTENDED` admin commands (after `sv_login`), allowing operators to compare current server connections against configured capacity directly from the admin console. Works for both static and dynamic (auth_query) pools.

- **PAUSE, RESUME, RECONNECT admin commands**: New admin console commands for managing connection pools. `PAUSE [db]` blocks new backend connection acquisition (active transactions continue). `RESUME [db]` lifts the pause and unblocks waiting clients. `RECONNECT [db]` forces connection rotation by incrementing the pool epoch — idle connections are immediately closed and active connections are discarded when returned to the pool. Without arguments, all pools are affected; with a database name, only matching pools. Specifying a nonexistent database returns an error. Use `SHOW POOLS` to see the `paused` status column.

- **`min_pool_size` for dynamic auth_query passthrough pools**: New `auth_query.min_pool_size` setting controls the minimum number of backend connections maintained per dynamic user pool in passthrough mode. Connections are prewarmed in the background when the pool is first created and replenished by the retain cycle after `server_lifetime` expiry. Pools with `min_pool_size > 0` are never garbage-collected. Default is `0` (no prewarm — backward compatible). Note: total backend connections scale as `active_users × min_pool_size`.

### 3.3.1 <small>Feb 26, 2026</small>

**Bug Fixes:**

- **Fix Ctrl+C in foreground mode**: Pressing Ctrl+C in foreground mode (with TTY attached) now performs a clean graceful shutdown instead of triggering a binary upgrade. Previously, each Ctrl+C would spawn a new pg_doorman process via `--inherit-fd`, leaving orphan processes accumulating. SIGINT in daemon mode (no TTY) retains its legacy binary upgrade behavior for backward compatibility with existing `systemd` units.

- **Minimum pool size enforcement (`min_pool_size`)**: The `min_pool_size` user setting is now enforced at runtime. After each connection retain cycle, pg_doorman checks pool sizes and creates new connections to maintain the configured minimum. Previously, `min_pool_size` was accepted in config but never applied — pools started empty and could drop to 0 connections even with `min_pool_size` set. Replenishment stops on the first connection failure to avoid hammering an unavailable server.

**New Features:**

- **SIGUSR2 for binary upgrade**: New dedicated signal `SIGUSR2` triggers binary upgrade + graceful shutdown in all modes (daemon and foreground). This is now the recommended signal for binary upgrades. The `systemd` service file has been updated to use `SIGUSR2` for `ExecReload`.

- **`UPGRADE` admin command**: New admin console command that triggers binary upgrade via SIGUSR2. Use it from `psql` connected to the admin database: `UPGRADE;`.

**Improvements:**

- **Pool prewarm at startup**: When `min_pool_size` is configured, pg_doorman now creates the minimum number of connections immediately at startup, before the first retain cycle. Previously, pools started empty and connections were only created lazily on first client request or after the first retain interval (default 60s). This eliminates cold-start latency for the first clients connecting after pg_doorman restart.

- **Configurable connection scaling parameters**: New `general` settings `scaling_warm_pool_ratio`, `scaling_fast_retries`, and `scaling_cooldown_sleep` allow tuning connection pool scaling behavior. All three can be overridden at the pool level. `scaling_cooldown_sleep` uses the human-readable `Duration` type (e.g. `"10ms"`, `"1s"`) consistent with other timeout fields.

- **`max_concurrent_creates` setting**: Controls the maximum number of server connections that can be created concurrently per pool. Uses a semaphore instead of a mutex for parallel connection creation.

### 3.3.0 <small>Feb 23, 2026</small>

**New Features:**

- **Dynamic user authentication (`auth_query`)**: PgDoorman can now authenticate users dynamically by querying PostgreSQL at connection time — no need to list every user in the config. Supports `pg_shadow`, custom tables, and `SECURITY DEFINER` functions. The query must return a column named `passwd` or `password` (or any single column) containing an MD5 or SCRAM-SHA-256 hash.

- **Passthrough authentication**: Default mode for both static and dynamic users — PgDoorman reuses the client's cryptographic proof (MD5 hash or SCRAM ClientKey) to authenticate to the backend automatically. No plaintext `server_password` in config needed when the pool user matches the backend PostgreSQL user.

- **Two auth_query modes**:
  - *Passthrough mode* (default) — each dynamic user gets their own backend connection pool and authenticates as themselves, preserving per-user identity on the backend.
  - *Dedicated mode* (`server_user` set) — all dynamic users share a single backend pool under one PostgreSQL role.

- **Auth query caching**: DashMap-based cache with configurable TTL, double-checked locking, rate-limited refetch, and request coalescing. Supports separate TTLs for successful and failed lookups.

- **`SHOW AUTH_QUERY` admin command**: Displays per-pool metrics — cache entries/hits/misses, auth success/failure counters, executor stats, and dynamic pool count.

- **Prometheus metrics for auth_query**: New metric families `pg_doorman_auth_query_cache`, `pg_doorman_auth_query_auth`, `pg_doorman_auth_query_executor`, `pg_doorman_auth_query_dynamic_pools`.

- **Idle dynamic pool garbage collection**: Background task cleans up expired dynamic pools when all connections have been idle beyond `server_lifetime`. Zero overhead for static-only configs.

- **Smart password column lookup**: Password column resolved by name (`passwd` → `password` → single-column fallback), works with `pg_shadow`, custom tables, and arbitrary single-column queries.

**Improvements:**

- **`server_username`/`server_password` now optional**: Previously documented as required for MD5/SCRAM hash configs. Now only needed when the backend user differs from the pool user (username mapping, JWT auth).

- **Data-driven config & docs generation**: `fields.yaml` is the single source of truth for all config field descriptions (EN/RU). Reference docs, annotated configs, and inline comments are all generated from it.

**Testing:**

- **39 new BDD scenarios** (260+ steps) covering auth_query executor, end-to-end auth, HBA integration, passthrough mode, SCRAM-only auth, RELOAD/GC lifecycle, observability, and static user passthrough.

### 3.2.4 <small>Feb 20, 2026</small>
**New Features:**

- **Annotated config generation**: The `generate` command now produces well-documented configuration files with inline comments for every parameter by default. Previously it only did plain serde serialization without any documentation.

- **`--reference` flag**: Generates a complete reference config with example values without requiring a PostgreSQL connection. The root `pg_doorman.toml` and `pg_doorman.yaml` are now auto-generated from this flag, ensuring they always stay in sync with the codebase.

- **`--format` (`-f`) flag**: Explicitly choose output format (`yaml` or `toml`). Default output format changed from TOML to YAML. When `--output` is specified, format is auto-detected from file extension; `--format` overrides auto-detection.

- **`--russian-comments` (`--ru`) flag**: Generates comments in Russian for quick start guide. All ~100+ comment strings are translated to clear, simple Russian.

- **`--no-comments` flag**: Disables inline comments for minimal config output (plain serde serialization, the old default behavior).

- **Passthrough authentication documentation**: Documents passthrough auth as the default mode — `server_username`/`server_password` are no longer needed when the pool user matches the backend PostgreSQL user. PgDoorman reuses the client's MD5 hash or SCRAM ClientKey to authenticate to the backend automatically.

**Testing:**

- **Config field coverage guarantee**: New test parses config struct source files (`general.rs`, `pool.rs`, `user.rs`, etc.) at compile time and verifies every `pub` field appears in annotated output. If someone adds a new config parameter but forgets to add it to `annotated.rs`, CI will fail with a clear message listing the missing fields.

- **BDD tests for generate command**: End-to-end tests that generate TOML and YAML configs, start pg_doorman with them, and verify client connectivity.

**Bug Fixes:**

- **Fixed protocol desynchronization on prepared statement cache eviction in async mode**: When asyncpg/SQLAlchemy uses `Flush` (instead of `Sync`) for pipelined `Parse+Describe` batches and the prepared statement LRU cache is full, eviction sends `Close+Sync` to the server. In async mode, `recv()` was exiting immediately when `expected_responses==0`, leaving `CloseComplete` and `ReadyForQuery` unread in the TCP buffer. The next `recv()` call would then read these stale messages instead of the expected response, causing protocol desynchronization. Fixed by temporarily disabling async mode during eviction so that `recv()` waits for `ReadyForQuery` as the natural loop terminator.

- **Fixed generated config startup failure**: `syslog_prog_name` and `daemon_pid_file` are now commented out by default in generated configs. Previously they were uncommented, causing pg_doorman to fail when started in foreground mode or when syslog was unavailable.

- **Fixed Go test goroutine leak**: `TestLibPQPrepared` now uses `sync.WaitGroup` to wait for all goroutines before test exit, fixing sporadic panics caused by logging after test completion.

- **Fixed protocol violation on flush timeout — client now receives ErrorResponse**: When the 5-second flush timeout fires (server TCP write blocks because the backend is overloaded or unreachable), the `FlushTimeout` error was propagating via `?` through `handle_sync_flush` → transaction loop → `handle()` without sending any PostgreSQL protocol message to the client. The TCP connection was simply dropped, causing drivers like Npgsql to report "protocol violation" due to unexpected EOF. Now pg_doorman sends a proper `ErrorResponse` with SQLSTATE `58006` and message containing "pooler is shut down now" before closing the connection, allowing client drivers to detect the error and reconnect gracefully.

### 3.2.3 <small>Feb 10, 2026</small>

**Improvements:**

- **Jitter for `server_lifetime` (±20%)**: Connection lifetimes now have a random ±20% jitter applied to prevent mass disconnections from PostgreSQL. When pg_doorman is under heavy load, it creates many connections simultaneously, which previously caused them all to expire at the same time, creating spikes of connection closures. Now each connection gets an individual lifetime calculated as `base_lifetime ± random(20%)`. For example, with `server_lifetime: 300000` (5 minutes), actual lifetimes range from 240s to 360s, spreading connection closures evenly over time.

### 3.2.2 <small>Feb 9, 2026</small>

**New Features:**

- **Configuration test mode (`-t` / `--test-config`)**: Added nginx-style configuration validation flag. Running `pg_doorman -t` or `pg_doorman --test-config` will parse and validate the configuration file, report success or errors, and exit without starting the server. Useful for CI/CD pipelines and pre-deployment configuration checks.

- **Configuration validation before binary upgrade**: When receiving SIGINT for graceful shutdown/binary upgrade, the server now validates the new binary's configuration using `-t` flag before proceeding. If the configuration test fails, the shutdown is cancelled and critical error messages are logged to alert the operator. This prevents accidental downtime from deploying a binary with invalid configuration.

- **New `retain_connections_max` configuration parameter**: Controls the maximum number of idle connections to close per retain cycle. When set to `0`, all idle connections that exceed `idle_timeout` or `server_lifetime` are closed immediately. Default is `3`, providing controlled cleanup while preventing connection buildup. Previously, only 1 connection was closed per cycle, which could lead to slow connection cleanup when many connections became idle simultaneously. Connection closures are now logged for better observability.

- **Oldest-first connection closure**: When `retain_connections_max > 0`, connections are now closed in order of age (oldest first) rather than in queue order. This ensures that the oldest connections are always prioritized for closure, providing more predictable connection rotation behavior.

- **New `server_idle_check_timeout` configuration parameter**: Time after which an idle server connection should be checked before being given to a client (default: 30s). This helps detect dead connections caused by PostgreSQL restart, network issues, or server-side idle timeouts. When a connection has been idle longer than this timeout, pg_doorman sends a minimal query (`;`) to verify the connection is alive before returning it to the client. Set to `0` to disable.

- **New `tcp_user_timeout` configuration parameter**: Sets the `TCP_USER_TIMEOUT` socket option for client connections (in seconds). This helps detect dead client connections faster than keepalive probes when the connection is actively sending data but the remote end has become unreachable. Prevents 15-16 minute delays caused by TCP retransmission timeout. Only supported on Linux. Default is `60` seconds. Set to `0` to disable.

- **Removed `wait_rollback` mechanism**: The pooler no longer attempts to automatically wait for ROLLBACK from clients when a transaction enters an aborted state. This complex mechanism was causing protocol desynchronization issues with async clients and extended query protocol. Server connections in aborted transactions are now simply returned to the pool and cleaned up normally via ROLLBACK during checkin.

- **Removed savepoint tracking**: Removed the `use_savepoint` flag and related logic that was tracking SAVEPOINT usage. The pooler now treats savepoints as regular PostgreSQL commands without special handling.

**Bug Fixes:**

- **Fixed protocol desynchronization in async mode with simple prepared statements**: When `prepared_statements` was disabled but clients used extended query protocol (Parse, Bind, Describe, Execute, Flush), the pooler wasn't tracking batch operations, causing `expected_responses` to be calculated as 0. This led to the pooler exiting the response loop immediately without waiting for server responses (ParseComplete, BindComplete, etc.). Now batch operations are tracked regardless of the `prepared_statements` setting.

**Performance:**

- **Removed timeout-based waiting in async protocol**: The pooler now tracks expected responses based on batch operations (Parse, Bind, Execute, etc.) and exits immediately when all responses are received. This eliminates unnecessary latency in pipeline/async workloads.

### 3.1.8 <small>Jan 31, 2026</small>

**Bug Fixes:**

- **Fixed ParseComplete desynchronization in pipeline on errors**: Fixed a protocol desynchronization issue (especially noticeable in .NET Npgsql driver) where synthetic `ParseComplete` messages were not being inserted if an error occurred during a pipelined batch. When the pooler caches a prepared statement and skips sending `Parse` to the server, it must still provide a `ParseComplete` to the client. If an error occurs before subsequent commands are processed, the server skips them, and the pooler now ensures all missing synthetic `ParseComplete` messages are inserted into the response stream upon receiving an `ErrorResponse` or `ReadyForQuery`.

- **Fixed incorrect `use_savepoint` state persistence**: Fixed a bug where the `use_savepoint` flag (which disables automatic rollback on connection return if a savepoint was used) was not reset after a transaction ended.


### 3.1.7 <small>Jan 28, 2026</small>

**Memory Optimization:**

- **DEALLOCATE now clears client prepared statements cache**: When a client sends `DEALLOCATE <name>` or `DEALLOCATE ALL` via simple query protocol, the pooler now properly clears the corresponding entries from the client's internal prepared statements cache. Previously, synthetic OK responses were sent but the client cache was not cleared, causing memory to grow indefinitely for long-running connections using many unique prepared statements. This fix allows memory to be reclaimed when clients properly deallocate their statements.

- **New `client_prepared_statements_cache_size` configuration parameter**: Added protection against malicious or misbehaving clients that don't call `DEALLOCATE` and could exhaust server memory by creating unlimited prepared statements. When the per-client cache limit is reached, the oldest entry is evicted automatically. Set to `0` for unlimited (default, relies on client calling `DEALLOCATE`). Example: `client_prepared_statements_cache_size: 1024` limits each client to 1024 cached prepared statements.

### 3.1.6 <small>Jan 27, 2026</small>

**Bug Fixes:**

- **Fixed incorrect timing statistics (xact_time, wait_time, percentiles)**: The statistics module was using `recent()` (cached clock) without proper clock cache updates, causing transaction time, wait time, and their percentiles to show extremely large incorrect values (e.g., 100+ seconds instead of actual milliseconds). The root cause was that the `quanta::Upkeep` handle was not being stored, causing the upkeep thread to stop immediately after starting. Now the handle is properly retained for the lifetime of the server, ensuring `Clock::recent()` returns accurate cached time values.

- **Fixed query time accumulation bug in transaction loop**: Query times were incorrectly accumulated when multiple queries were executed within a single transaction. The `query_start_at` timestamp was only set once at the beginning of the transaction, causing each subsequent query's elapsed time to include all previous queries' durations (e.g., 10 queries of 100ms each would report the last query as ~1 second instead of 100ms). Now `query_start_at` is updated for each new message in the transaction loop, ensuring accurate per-query timing.

**New Features:**

- **New `clock_resolution_statistics` configuration parameter**: Added `general.clock_resolution_statistics` parameter (default: `0.1ms` = 100 microseconds) that controls how often the internal clock cache is updated. Lower values provide more accurate timing measurements for query/transaction percentiles, while higher values reduce CPU overhead. This parameter affects the accuracy of all timing statistics reported in the admin console and Prometheus metrics.

- **Sub-millisecond precision for Duration values**: Duration configuration parameters now support sub-millisecond precision:
  - New `us` suffix for microseconds (e.g., `"100us"` = 100 microseconds)
  - Decimal milliseconds support (e.g., `"0.1ms"` = 100 microseconds)
  - Internal representation changed from milliseconds to microseconds for higher precision
  - Full backward compatibility maintained: plain numbers are still interpreted as milliseconds

### 3.1.5 <small>Jan 25, 2026</small>

**Bug Fixes:**

- **Fixed PROTOCOL VIOLATION with batch PrepareAsync**
- **Rewritten ParseComplete insertion algorithm**

**Performance:**

- **Deferred connection acquisition for standalone BEGIN**: When a client sends a standalone `BEGIN;` or `begin;` query (simple query protocol), the pooler now defers acquiring a server connection until the next message arrives. Since `BEGIN` itself doesn't perform any actual database operations, this optimization reduces connection pool contention when clients are slow to send their next query after starting a transaction.
  - Micro-optimized detection: first checks message size (12 bytes), then content using case-insensitive comparison
  - If client sends Terminate (`X`) after `BEGIN`, no server connection is acquired at all
  - The deferred `BEGIN` is automatically sent to the server before the actual query

### 3.1.0 <small>Jan 18, 2026</small>

**New Features:**

- **YAML configuration support**: Added support for YAML configuration files (`.yaml`, `.yml`) as the primary and recommended format. The format is automatically detected based on file extension. TOML format remains fully supported for backward compatibility.
  - The `generate` command now outputs YAML or TOML based on the output file extension.
  - Include files can mix YAML and TOML formats.
  - New array syntax for users in YAML: `users: [{ username: "user1", ... }]`
- **TOML backward compatibility**: Full backward compatibility with legacy TOML format `[pools.*.users.0]` is maintained. Both the legacy map format and the new array format `[[pools.*.users]]` are supported.
- **Username uniqueness validation**: Added validation to reject duplicate usernames within a pool, ensuring configuration correctness.
- **Human-readable configuration values**: Duration and byte size parameters now support human-readable formats while maintaining backward compatibility with numeric values:
  - Duration: `"3s"`, `"5m"`, `"1h"`, `"1d"` (or milliseconds: `3000`)
  - Byte size: `"1MB"`, `"256M"`, `"1GB"` (or bytes: `1048576`)
  - Example: `connect_timeout: "3s"` instead of `connect_timeout: 3000`
- **Foreground mode binary upgrade**: Added support for binary upgrade in foreground mode by passing the listener socket to the new process via `--inherit-fd` argument. This enables zero-downtime upgrades without requiring daemon mode.
- **Optional tokio runtime parameters**: The following tokio runtime parameters are now optional and default to `None` (using tokio's built-in defaults): `tokio_global_queue_interval`, `tokio_event_interval`, `worker_stack_size`, and the new `max_blocking_threads`. Modern tokio versions handle these parameters well by default, so explicit configuration is no longer required in most cases.
- **Improved graceful shutdown behavior**:
  - During graceful shutdown, only clients with active transactions are now counted (instead of all connected clients), allowing faster shutdown when clients are idle.
  - After a client completes their transaction during shutdown, they receive a proper PostgreSQL protocol error (`58006 - pooler is shut down now`) instead of a connection reset.
  - Server connections are immediately released (marked as bad) after transaction completion during shutdown to conserve PostgreSQL connections.
  - All idle connections are immediately drained from pools when graceful shutdown starts, releasing PostgreSQL connections faster.

**Performance:**

- **Statistics module optimization**: Major refactoring of the `src/stats` module for improved performance:
  - Replaced `VecDeque` with HDR histograms (`hdrhistogram` crate) for percentile calculations — O(1) percentile queries instead of O(n log n) sorting, ~95% memory reduction for latency tracking.
  - Histograms are now reset after each stats period (15 seconds) to provide accurate rolling window percentiles.

### 3.0.5 <small>Jan 16, 2026</small>

**Bug Fixes:**

- Fixed panic (`capacity overflow`) in startup message handling when receiving malformed messages with invalid length (less than 8 bytes or exceeding 10MB). Now gracefully rejects such connections with `ClientBadStartup` error.

**Testing:**

- **Integration fuzz tests**: Added BDD fuzz tests (`@fuzz` tag) for malformed PostgreSQL protocol messages.
- All fuzz tests connect and authenticate first, then send malformed data to test post-authentication resilience.

**CI/CD:**

- Added dedicated fuzz test job in GitHub Actions workflow (without retries, as fuzz tests should not be flaky).

### 3.0.4 <small>Jan 16, 2026</small>

**New Features:**

- **Enhanced DEBUG logging for PostgreSQL protocol messages**: Added grouped debug logging that displays message types in a compact format (e.g., `[P(stmt1),B,D,E,S]` or `[3xD,C,Z]`). Messages are buffered and flushed every 100ms or 100 messages to reduce log noise.
- **Protocol violation detection**: Added real-time protocol state tracking that detects and warns about protocol violations (e.g., receiving ParseComplete when no Parse was pending). Helps diagnose client-server synchronization issues.

**Bug Fixes:**

- Fixed potential protocol violation when client disconnects during batch operations with cached prepared statements: disabled fast_release optimization when there are pending prepared statement operations.
- Fixed ParseComplete insertion for Describe flow: now correctly inserts one ParseComplete before each ParameterDescription ('t') or NoData ('n') message instead of inserting all at once.

### 3.0.3 <small>Jan 15, 2026</small>

**Bug Fixes:**

- Improved handling of Describe flow for cached prepared statements: added a separate counter (`pending_parse_complete_for_describe`) to correctly insert ParseComplete messages before ParameterDescription or NoData responses when Parse was skipped due to caching.

**Testing:**

- Added .NET client tests for Describe flow with cached prepared statements (`describe_flow_cached.cs`).
- Added mixed tests combining batch operations, prepared statements, and extended protocol (`aggressive_mixed.cs`).

### 3.0.2 <small>Jan 14, 2026</small>

**Bug Fixes:**

- Fixed protocol mismatch for .NET clients (Npgsql) using named prepared statements with `Prepare()`: ParseComplete messages are now correctly inserted before ParameterDescription and NoData messages in the Describe flow, not just before BindComplete.

### 3.0.1 <small>Jan 14, 2026</small>

**Bug Fixes:**

- Fixed protocol mismatch for .NET clients (Npgsql): prevented insertion of ParseComplete messages between DataRow messages when server has more data available.

**Testing:**

- Extended Node.js client test coverage with additional scenarios for prepared statements, error handling, transactions, and edge cases.

### 3.0.0 <small>Jan 12, 2026</small>

**Architecture refactor**

PgDoorman 3.0.0 reorganizes the client, config, admin, auth, and
prometheus modules, and adds the `patroni_proxy` binary.

**New Features:**

- **patroni_proxy** — a TCP proxy for Patroni-managed PostgreSQL clusters:
    - Zero-downtime connection management — existing connections are preserved during cluster topology changes
    - Hot upstream updates — automatic discovery of cluster members via Patroni REST API without connection drops
    - Role-based routing — route connections to leader, sync replicas, or async replicas based on configuration
    - Replication lag awareness with configurable `max_lag_in_bytes` per port
    - Least connections load balancing strategy

**Improvements:**

- **Module split**:
    - Client handling split into dedicated modules (core, entrypoint, protocol, startup, transaction)
    - Configuration system reorganized into focused modules (general, pool, user, tls, prometheus, talos)
    - Admin, auth, and prometheus subsystems extracted into separate modules
- **Async protocol support** — improved handling of asynchronous PostgreSQL protocol messages.
- **Extended protocol** — improved client buffering and message handling.
- **xxhash3 for prepared statement hashing** — faster hash computation for prepared statement cache
- **BDD test framework** — multi-language integration tests (Go, Rust, Python, Node.js, .NET) in a Docker-based environment.

### 2.5.0 <small>Nov 18, 2025</small>

**Improvements:**
- Reworked the statistics collection system, yielding up to 20% performance gain on fast queries.
- Improved detection of `SAVEPOINT` usage, allowing the auto-rollback feature to be applied in more situations.

**Bug Fixes / Behavior:**
- Less aggressive behavior on write errors when sending a response to the client: the server connection is no longer immediately marked as "bad" and evicted from the pool. We now read the remaining server response and clean up its state, returning the connection to the pool in a clean state. This improves performance during client reconnections.


### 2.4.3 <small>Nov 15, 2025</small>

**Bug Fixes:**
- Fixed handling of nested transactions via `SAVEPOINT`: auto-rollback now correctly rolls back to the savepoint instead of breaking the outer transaction. This prevents clients from getting stuck in an inconsistent transactional state.


### 2.4.2 <small>Nov 13, 2025</small>

**Improvements:**
- `pg_hba` rules now apply to the admin console as well; the `trust` method can be used for admin connections when a matching rule is present (use with caution; restrict by address/TLS).

**Bug Fixes:**
- Fixed `pg_hba` evaluation: `local` records were mistakenly considered; PgDoorman only handles TCP connections, so `local` entries are now correctly ignored.



### 2.4.1 <small>Nov 12, 2025</small>

**Improvements:**
- Performance optimizations in request handling and message processing paths to reduce latency and CPU usage.
- `pg_hba` rules now apply to the admin console as well; the `trust` method can be used for admin connections when a matching rule is present (use with caution; restrict by address/TLS).

**Bug Fixes:**
- Corrected logic where `COMMIT` could be mishandled similarly to `ROLLBACK` in certain error states; now transactional state handling is aligned with PostgreSQL semantics.


### 2.4.0 <small>Nov 10, 2025</small>

**Features:**
- Added `pg_hba` support to control client access in PostgreSQL format. New `general.pg_hba` setting supports inline content or file path.
- Clients that enter the `aborted in transaction` state are detached from their server backend; the proxy waits for the client to send `ROLLBACK`.

**Improvements:**
- Refined admin and metrics counters: separated `cancel` connections and corrected calculation of `error` connections in admin output and Prometheus metrics descriptions.
- Added configuration validation to prevent simultaneous use of legacy `general.hba` CIDR list with the new `general.pg_hba` rules.
- Improved validation and error messages for Talos token authentication.

### 2.2.2 <small>Aug 17, 2025</small>

**Features:**
- Added new generate feature functionality

**Bug Fixes:**
- Fixed deallocate issues with PGX5 compatibility

### 2.2.1 <small>Aug 6, 2025</small>

**Features:**
- Improve Prometheus exporter functionality

### 2.2.0 <small>Aug 5, 2025</small>

**Features:**
- Added Prometheus exporter functionality that provides metrics about connections, memory usage, pools, queries, and transactions

### 2.1.2 <small>Aug 4, 2025</small>

**Features:**
- Added docker image `ghcr.io/ozontech/pg_doorman`


### 2.1.0 <small>Aug 1, 2025</small>

**Features:**
- The new command `generate` connects to your PostgreSQL server, automatically detects all databases and users, and creates a complete configuration file with appropriate settings. This is especially useful for quickly setting up PgDoorman in new environments or when you have many databases and users to configure.


### 2.0.1 <small>July 24, 2025</small>

**Bug Fixes:**
- Fixed `max_memory_usage` counter leak when clients disconnect improperly.

### 2.0.0 <small>July 22, 2025</small>

**Features:**
- Added `tls_mode` configuration option to enhance security with flexible TLS connection management and client certificate validation capabilities.

### 1.9.0 <small>July 20, 2025</small>

**Features:**
- Added PAM authentication support.
- Added `talos` JWT authentication support.

**Improvements:**
- Implemented streaming for COPY protocol with large columns to prevent memory exhaustion.
- Updated Rust and Tokio dependencies.

### 1.8.3 <small>Jun 11, 2025</small>

**Bug Fixes:**
- Fixed critical bug where Client's buffer wasn't cleared when no free connections were available in the Server pool (query_wait_timeout), leading to incorrect response errors. [#38](https://github.com/ozontech/pg_doorman/pull/38)
- Fixed Npgsql-related issue. [Npgsql#6115](https://github.com/npgsql/npgsql/issues/6115)

### 1.8.2 <small>May 24, 2025</small>

**Features:**
- Added `application_name` parameter in pool. [#30](https://github.com/ozontech/pg_doorman/pull/30)
- Added support for `DISCARD ALL` and `DEALLOCATE ALL` client queries.

**Improvements:**
- Implemented link-time optimization. [#29](https://github.com/ozontech/pg_doorman/pull/29)

**Bug Fixes:**
- Fixed panics in admin console.
- Fixed connection leakage on improperly handled errors in client's copy mode.

### 1.8.1 <small>April 12, 2025</small>

**Bug Fixes:**
- Fixed config value of prepared_statements. [#21](https://github.com/ozontech/pg_doorman/pull/21)
- Fixed handling of declared cursors closure. [#23](https://github.com/ozontech/pg_doorman/pull/23)
- Fixed proxy server parameters. [#25](https://github.com/ozontech/pg_doorman/pull/25)

### 1.8.0 <small>Mar 20, 2025</small>

**Bug Fixes:**
- Fixed dependencies issue. [#15](https://github.com/ozontech/pg_doorman/pull/15)

**Improvements:**
- Added release vendor-licenses.txt file. [Related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com)

### 1.7.9 <small>Mar 16, 2025</small>

**Improvements:**
- Added release vendor.tar.gz for offline build. [Related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com)

**Bug Fixes:**
- Fixed issues with pqCancel messages over TLS protocol. Drivers should send pqCancel messages exclusively via TLS if the primary connection was established using TLS. [Npgsql](https://github.com/npgsql/npgsql) follows this rule, while [PGX](https://github.com/jackc/pgx) currently does not. Both behaviors are now supported.

### 1.7.8 <small>Mar 8, 2025</small>

**Bug Fixes:**
- Fixed message ordering issue when using batch processing with the extended protocol.
- Improved error message detail in logs for server-side login attempt failures.

### 1.7.7 <small>Mar 8, 2025</small>

**Features:**
- Enhanced `show clients` command with new fields: `state` (waiting/idle/active) and `wait` (read/write/idle).
- Enhanced `show servers` command with new fields: `state` (login/idle/active), `wait` (read/write/idle), and `server_process_pid`.
- Added 15-second proxy timeout for streaming large `message_size_to_be_stream` responses.

**Bug Fixes:**
- Fixed `max_memory_usage` counter leak when clients disconnect improperly.
