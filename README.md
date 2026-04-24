![pg_doorman](/static/logo_color_bg.png)

# PgDoorman

[![BDD Tests](https://github.com/ozontech/pg_doorman/actions/workflows/bdd-tests.yml/badge.svg)](https://github.com/ozontech/pg_doorman/actions/workflows/bdd-tests.yml)
[![Library Tests](https://github.com/ozontech/pg_doorman/actions/workflows/lib-tests.yml/badge.svg)](https://github.com/ozontech/pg_doorman/actions/workflows/lib-tests.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A high-performance multithreaded PostgreSQL connection pooler built in Rust. Does one thing and does it well — pools connections so your PostgreSQL handles thousands of clients without breaking a sweat.

## Why PgDoorman?

**Drop-in replacement. No app changes.** PgDoorman caches and remaps prepared statements transparently across server connections in transaction mode — just point your connection string at it and go. No `DISCARD ALL`, no `DEALLOCATE`, no driver hacks. PgBouncer added similar support in 1.21, but remains single-threaded; Odyssey added it in 1.3, but has known reliability issues in edge cases.

**Battle-tested with real drivers.** Two years of production use with Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), Node.js. Protocol edge cases — pipelined batches, async Flush, Describe flow, cancel requests over TLS — are covered by comprehensive multi-language BDD tests.

**Natively multithreaded.** PgBouncer is single-threaded. Running multiple instances via `SO_REUSE_PORT` leads to unbalanced pools: clients connect evenly but disconnect unpredictably, leaving some instances overloaded while others sit idle. PgDoorman uses a single shared pool across all worker threads, ensuring correct connection distribution at any scale.

**Full extended query protocol support.** Benchmarks show Odyssey is up to 61% slower with the extended query protocol in transaction mode. Odyssey also has known crashes under query cancellation stress and segfaults on large packets. PgDoorman handles simple, extended, and prepared protocols equally well — including pipelined batches and async Flush flow that cause issues in other poolers.

**Built for operations.** `pg_doorman generate --host your-db` creates a config by introspecting PostgreSQL — no manual user/database enumeration. `pg_doorman -t` validates config before deploy (PgBouncer and Odyssey lack this). YAML config with human-readable durations (`"30s"`, `"5m"`, `"1h"`). Built-in Prometheus endpoint — no external exporter needed (PgBouncer requires a separate process; Odyssey's built-in metrics segfault when combined with standard logging).

**Database-level connection limits with eviction.** `max_db_connections` caps total server connections per database. When the limit is reached, idle connections are evicted from users with the largest surplus — ranked by p95 transaction time so slow pools donate first, respecting per-user minimums. A reserve pool absorbs short bursts without evicting anyone. PgBouncer has `max_db_connections` but without eviction, fairness, or reserve. Odyssey has no equivalent.

**Thundering herd suppression.** When 200 clients request a connection and the pool has 4 idle, a naive pooler fires 196 backend `connect()` calls at once — saturating PostgreSQL's `accept()` queue and `pg_authid` lookups. pg_doorman caps concurrent backend creates at `scaling_max_parallel_creates` (default 2) per pool and routes waiting callers to reuse connections being returned by other clients via direct handoff. Most clients land on a recycled connection within microseconds instead of waiting for a fresh `connect()`.

**Bounded tail latency.** The pool hands returned connections to the longest-waiting client first (strict FIFO via direct-handoff channels). This keeps p99 latency within 10% of p50 regardless of client count. Poolers that use broadcast-notify or LIFO scheduling show 10-25x p99/p50 ratios under contention — some clients get instant service while others starve. When connections approach `server_lifetime` expiry, a replacement is created in the background before the old one dies — zero latency spike during rotation. [Latency breakdown by percentile](https://ozontech.github.io/pg_doorman/benchmarks.html).

**Dead backend detection.** When a client holds a transaction open, pg_doorman probes the backend and returns an error immediately if the server is gone (failover, OOM kill). Other poolers rely on TCP keepalive, leaving clients hanging for minutes.

## Benchmarks

Automated benchmarks on AWS Fargate (16 vCPU, pool size 40, pgbench 30s per test):

| Scenario | vs PgBouncer | vs Odyssey |
|----------|-------------|------------|
| Extended protocol, 500 clients + SSL | x3.5 | +61% |
| Prepared statements, 500 clients + SSL | x4.0 | +5% |
| Simple protocol, 10,000 clients | x2.8 | +20% |
| Extended + SSL + Reconnect, 500 clients | +96% | ~0% |

PgBouncer is single-threaded — these ratios reflect a single PgBouncer instance vs a single PgDoorman instance. [Full benchmark results](https://ozontech.github.io/pg_doorman/benchmarks.html).

## Comparison

| | PgDoorman | PgBouncer | Odyssey |
|---|:-:|:-:|:-:|
| Multithreaded | Yes | No | Yes |
| Prepared statements in transaction mode | Yes | Since 1.21 | Since 1.3 |
| Full extended query protocol | Yes | Yes | Partial |
| Deferred `BEGIN` (lazy server acquire) | Yes | No | No |
| Stale backend detection (idle-in-transaction) | Yes | No | No |
| Zero-downtime binary upgrade | Yes | Yes | Yes |
| Config test mode (`-t` / `--test-config`) | Yes | No | No |
| Auto-config from PostgreSQL | Yes | No | No |
| YAML / TOML config | Yes | No (INI) | No (own format) |
| Human-readable durations & sizes | Yes | No | No |
| Native `pg_hba.conf` format | Yes | Yes | Since 1.4 |
| Auth query (dynamic users) | Yes | Yes | Yes |
| Auth query passthrough (per-user backend identity) | Yes | No | Yes |
| PAM auth | Yes | Yes | Yes |
| LDAP auth | No | Since 1.25 | Yes |
| Database-level connection limits (`max_db_connections`) | Yes | Yes (no eviction) | No |
| Pre-replacement on `server_lifetime` expiry | Yes | No | No |
| Runtime log level control (`SET log_level`) | Yes | No | No |
| PAUSE / RESUME / RECONNECT | Yes | Yes | Yes |
| TLS: minimum TLS 1.2, Mozilla ciphers | Yes | Yes | No (allows TLS 1.0, weak ciphers) |
| Prometheus metrics | Built-in | External | Built-in |

## Quick Start

### Minimal config

```yaml
general:
  host: "0.0.0.0"
  port: 6432
  admin_username: "admin"
  admin_password: "change_me"

pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "app"
        password: "md5..."           # hash from pg_shadow / pg_authid
        pool_size: 40
```

> **Passthrough authentication (default):** When `server_username` and `server_password` are omitted, PgDoorman reuses the client's cryptographic proof (MD5 hash or SCRAM ClientKey) to authenticate to PostgreSQL automatically. This is the recommended setup when the pool username matches the backend PostgreSQL user — no plaintext passwords in config needed.
>
> Set `server_username` / `server_password` only when the backend user differs from the pool user (e.g., username mapping) or for JWT authentication where there is no password to pass through.

### Auth query (dynamic users)

Instead of listing every user in the config, pg_doorman can look up credentials directly from PostgreSQL. The query must return a column named `passwd` or `password` containing the MD5 or SCRAM hash. Any extra columns are ignored.

Quickstart — using `pg_shadow` directly (requires superuser):

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    auth_query:
      query: "SELECT passwd FROM pg_shadow WHERE usename = $1"
      user: "postgres"
      password: "postgres_password"
```

By default auth_query runs in **passthrough mode**: each dynamic user gets their own backend pool and authenticates as themselves. To force all users through a single backend role, set `server_user` / `server_password` (dedicated mode).

> Static users (defined in `users`) are checked first. auth_query is only consulted when the username is not found among static users.

> **Production:** don't use superuser for auth queries. Create a [`SECURITY DEFINER` function](https://ozontech.github.io/pg_doorman/reference/pool.html#auth-query-settings) with a dedicated low-privilege role instead.

Or generate a config automatically:

```bash
pg_doorman generate --host your-db-host --output pg_doorman.yaml
```

### Run and connect

```bash
# Start
pg_doorman pg_doorman.yaml

# Connect — same as you would to PostgreSQL directly
psql -h localhost -p 6432 -U app mydb
```

Your application connection string changes only the host and port:

```
postgresql://app:secret@localhost:6432/mydb
```

## Pooling Modes

PgDoorman supports two pooling modes, configured per pool or per user:

**Transaction mode** (default, recommended) — server connection is acquired when a transaction starts and released back to the pool when it ends. One backend serves many clients, giving the best connection utilization.

**Session mode** — server connection is held for the entire client session. Use this when your application relies on session-level features like `LISTEN/NOTIFY`, temporary tables, or advisory locks.

```yaml
pools:
  mydb:
    pool_mode: "transaction"   # or "session"
```

## SQL Feature Compatibility

What works in each pooling mode:

| Feature | Transaction | Session |
|---------|:-----------:|:-------:|
| Regular queries (SELECT, INSERT, ...) | Yes | Yes |
| Prepared statements (Parse/Bind/Execute) | Yes (transparent caching) | Yes |
| SET / RESET | Yes (auto-RESET ALL on checkin) | Yes |
| Cursors (DECLARE / FETCH / CLOSE) | Yes (auto-CLOSE ALL on checkin) | Yes |
| LISTEN / NOTIFY | No — use session mode | Yes |
| Temporary tables | No — use session mode | Yes |
| Advisory locks (`pg_advisory_xact_lock`) | Yes (transaction-scoped) | Yes |
| Session-level advisory locks | No — use `pg_advisory_xact_lock` | Unreliable with pooling |
| DISCARD ALL | Yes | Yes |
| COPY | Yes | Yes |

In transaction mode, PgDoorman automatically cleans up server state (`RESET ALL`, `CLOSE ALL`) when returning a connection to the pool, so the next client gets a clean connection.

> **Advisory locks and connection pooling:** Session-level advisory locks (`pg_advisory_lock`) are unreliable with any connection pooler — the lock is tied to a backend connection, not to your application session, so another client may inherit or release it unexpectedly. Use transaction-level `pg_advisory_xact_lock()` instead, which is automatically released at transaction end and works correctly in transaction mode.

## Admin Commands

Connect to the admin console and manage pools at runtime:

```sql
-- Block new connections (active transactions continue)
PAUSE mydb;
PAUSE;          -- all pools

-- Unblock waiting clients
RESUME mydb;
RESUME;         -- all pools

-- Force backend connection rotation (epoch-based, no downtime)
RECONNECT mydb;
RECONNECT;      -- all pools
```

Full connection rotation pattern: `PAUSE → RECONNECT → RESUME`.

```sql
-- Trigger binary upgrade (zero downtime)
UPGRADE;
```

See [admin commands documentation](https://ozontech.github.io/pg_doorman/tutorials/basic-usage.html) for details.

## Database-level connection limits

Cap total server connections per database across all user pools. When the limit is reached, pg_doorman evicts idle connections from users with the largest surplus, waits for a connection to free up, and falls back to a reserve pool as last resort.

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    max_db_connections: 80        # hard cap (0 = disabled)
    min_connection_lifetime: 30000 # don't evict connections younger than 30s
    reserve_pool_size: 20         # extra slots beyond the limit
    reserve_pool_timeout: 3000    # wait up to 3s for a free connection before falling back to reserve
    min_guaranteed_pool_size: 2   # per-user eviction protection
    users:
      - username: "app"
        password: "md5..."
        pool_size: 60
      - username: "analytics"
        password: "md5..."
        pool_size: 80
```

Disabled by default — zero overhead when `max_db_connections` is 0 or omitted. `min_guaranteed_pool_size` protects each user from having all connections evicted (independent of `min_pool_size`, which controls prewarm). Monitor via `SHOW POOL_COORDINATOR` or the `pg_doorman_pool_coordinator` Prometheus metric.

## TLS / SSL

PgDoorman supports TLS encryption on both the client-facing and server-facing sides.

### Client-facing TLS

Encrypt connections between your application and PgDoorman:

```yaml
general:
  tls_certificate: "/path/to/server.crt"
  tls_private_key: "/path/to/server.key"
  tls_mode: "require"                      # disable | allow | require | verify-full
  # tls_ca_cert: "/path/to/ca.crt"        # required for verify-full
  # tls_rate_limit_per_second: 500         # limit TLS handshakes (0 = unlimited)
```

| Mode | Behavior |
|------|----------|
| `disable` | TLS not allowed |
| `allow` | TLS accepted but not required (default when cert is configured) |
| `require` | TLS required, client certificates not verified |
| `verify-full` | TLS required, client certificate verified against CA |

### Server-facing TLS

Encrypt connections from PgDoorman to PostgreSQL. Set `server_tls_mode` in `[general]` for a global default; override per pool as needed.

| Mode | Behavior |
|------|----------|
| `disable` | never use TLS |
| `allow` | try plain first; retry with TLS if server rejects plain (default, matches libpq `sslmode=allow`) |
| `prefer` | try TLS first; fall back to plain if server declines |
| `require` | require TLS, do not verify certificate |
| `verify-ca` | require TLS + verify server certificate against CA |
| `verify-full` | require TLS + verify CA + verify hostname |

```yaml
general:
  server_tls_mode: "verify-ca"
  server_tls_ca_cert: "/path/to/ca.crt"
  server_tls_certificate: "/path/to/client.crt"   # optional: mTLS client cert
  server_tls_private_key: "/path/to/client.key"   # optional: mTLS client key
```

All four fields can be overridden per pool:

```yaml
pools:
  mypool:
    server_tls_mode: "require"
    server_tls_ca_cert: "/path/to/other-ca.crt"
```

**Known limitations:**

- **`prefer` mode fallback:** If the server accepts the SSL request but the TLS handshake fails (e.g., cipher mismatch), the connection is not retried on plain TCP (unlike libpq). This edge case is rare in practice.
- **`channel_binding = require`:** PostgreSQL's `channel_binding = require` setting is incompatible with pg_doorman. The pooler uses separate TLS sessions for client-to-pooler and pooler-to-server connections, so SCRAM channel binding cannot be forwarded.
- **Cipher suites:** Cipher suite selection is not currently configurable; system OpenSSL defaults are used. TLS 1.2 is the minimum protocol version.

### Security defaults

PgDoorman enforces strict TLS defaults out of the box:

- **TLS 1.2 minimum** — TLS 1.0/1.1 are rejected (deprecated per RFC 8996)
- **Mozilla Intermediate cipher suites** — only modern AEAD ciphers (AES-GCM, ChaCha20-Poly1305) with forward secrecy (ECDHE/DHE); no RC4, DES, or CBC
- **Full hostname verification** — `verify-full` checks both Subject Alternative Names (SANs) and Common Name (CN) via OpenSSL's `verify_hostname()`; Odyssey only checks CN, which is [obsolete practice](https://datatracker.ietf.org/doc/html/rfc6125)
- **Startup validation** — certificates and keys are loaded and verified at startup, not at first connection

## Monitoring

Built-in Prometheus metrics endpoint — no external exporters needed.

```yaml
prometheus:
  enabled: true
  host: "0.0.0.0"
  port: 9127
```

Scrape `http://host:9127/` to collect metrics. Key metrics:

| Metric | Labels | Description                                 |
|--------|--------|---------------------------------------------|
| `pg_doorman_pools_clients` | status, user, database | Clients by status (active / idle / waiting) |
| `pg_doorman_pools_servers` | status, user, database | Servers by status (active / idle)           |
| `pg_doorman_pool_size` | user, database | Configured max pool size                    |
| `pg_doorman_pools_queries_count` | user, database | Total queries executed                      |
| `pg_doorman_pools_queries_percentile` | percentile, user, database | Query time p50 / p90 / p95 / p99 (ms)       |
| `pg_doorman_pools_transactions_count` | user, database | Total transactions executed                 |
| `pg_doorman_pools_avg_wait_time` | user, database | Avg client wait for server (ms)             |
| `pg_doorman_pools_bytes` | direction, user, database | Bytes sent / received                       |
| `pg_doorman_pool_prepared_cache_entries` | user, database | Pool-level prepared statement cache entries |
| `pg_doorman_pool_prepared_cache_bytes` | user, database | Pool-level prepared statement cache bytes   |
| `pg_doorman_clients_prepared_cache_entries` | user, database | Client-level prepared statement cache entries |
| `pg_doorman_async_clients_count` | user, database | Clients using async protocol (Flush)        |
| `pg_doorman_pools_transactions_percentile` | percentile, user, database | Transaction time p50 / p90 / p95 / p99 (ms) |
| `pg_doorman_servers_prepared_hits` | user, database, backend_pid | Per-server prepared statement cache hits   |
| `pg_doorman_servers_prepared_misses` | user, database, backend_pid | Per-server prepared statement cache misses |
| `pg_doorman_auth_query_cache` | type, database | Auth query cache entries / hits / misses / refetches / rate_limited |
| `pg_doorman_auth_query_auth` | result, database | Auth outcomes (success / failure)            |
| `pg_doorman_auth_query_executor` | type, database | Auth query executor queries / errors         |
| `pg_doorman_auth_query_dynamic_pools` | type, database | Dynamic pool lifecycle (current / created / destroyed) |
| `pg_doorman_pool_coordinator` | type, database | Coordinator stats (connections, reserve, evictions, exhaustions) |
| `pg_doorman_total_memory` | — | Process memory usage (bytes)                |
| `pg_doorman_connection_count` | type | Connections by type (plain / tls / total)   |

A ready-to-import [Grafana dashboard](grafana/) is included — pool utilization, latency percentiles, coordinator state, prepared statement cache, and auth query metrics.

## Signals & Zero-Downtime Upgrade

| Signal | Effect |
|--------|--------|
| `SIGHUP` | Reload configuration without restart |
| `SIGUSR2` | Start binary upgrade + graceful shutdown of old process |
| `SIGTERM` | Immediate shutdown |

### Binary upgrade with client migration (zero downtime)

Replace the pg_doorman binary while clients stay connected — no reconnects, no errors, no re-authentication:

```bash
# Replace the binary on disk, then:
kill -USR2 $(cat /tmp/pg_doorman.pid)

# Or from the admin console:
UPGRADE;
```

PgDoorman validates the new binary's configuration (`-t` flag) before starting it. If validation fails, the upgrade is aborted and the old process continues.

In foreground mode, idle clients are migrated to the new process via Unix socket fd passing (SCM_RIGHTS). The TCP connection stays the same — the client does not notice the upgrade. Clients inside a transaction finish on the old process and migrate after COMMIT. The old process exits when all clients have migrated or `shutdown_timeout` expires.

Prepared statement caches are serialized and transferred. The new process transparently re-issues `Parse` commands to fresh backends on the first `Bind` — clients do not need to re-prepare anything.

TLS clients can also be migrated without re-handshaking. Build with `--features tls-migration` to enable this: a patched OpenSSL 3.5.5 exports the symmetric cipher state (keys, IVs, sequence numbers) and the new process imports it to resume encryption. Linux only.

For systemd services:

```ini
ExecReload=/bin/kill -SIGUSR2 $MAINPID
```

See [binary upgrade documentation](https://ozontech.github.io/pg_doorman/tutorials/binary-upgrade.html) for the full protocol description, TLS migration build instructions, configuration, monitoring, and troubleshooting.

## Installation

**Pre-built binaries:** Download from [GitHub Releases](https://github.com/ozontech/pg_doorman/releases).

```bash
# Ubuntu/Debian
sudo add-apt-repository ppa:vadv/pg-doorman && sudo apt-get install pg-doorman

# Fedora/RHEL/Rocky
sudo dnf copr enable vadvya/pg-doorman && sudo dnf install pg-doorman

# Docker
docker pull ghcr.io/ozontech/pg_doorman
```

### Building from source

```bash
# Recommended: build with jemalloc tuning for optimal memory management
JEMALLOC_SYS_WITH_MALLOC_CONF="dirty_decay_ms:30000,muzzy_decay_ms:30000,background_thread:true,metadata_thp:auto" \
  cargo build --release

# Binary will be at target/release/pg_doorman
```

## Coming from PgBouncer?

PgDoorman uses YAML instead of INI, but the concepts are the same:

| PgBouncer (INI) | PgDoorman (YAML) | Notes |
|-----------------|-------------------|-------|
| `pool_mode = transaction` | `pool_mode: "transaction"` | Same semantics |
| `max_client_conn = 1000` | `general.max_connections: 1000` | |
| `default_pool_size = 20` | `users[].pool_size: 20` | Set per-user, not globally |
| `server_lifetime = 3600` | `general.server_lifetime: "1h"` | Human-readable durations |
| `server_idle_timeout = 600` | `general.idle_timeout: "10m"` | |
| `auth_query = ...` | `pools.<db>.auth_query.query: ...` | Same concept, YAML structure |
| `max_db_connections = 80` | `pools.<db>.max_db_connections: 80` | + eviction, reserve pool, per-user protection |
| `reserve_pool_size = 20` | `pools.<db>.reserve_pool_size: 20` | + priority-based distribution |
| `listen_addr = *` | `general.host: "0.0.0.0"` | |
| `listen_port = 6432` | `general.port: 6432` | |
| `admin_users = admin` | `general.admin_username: "admin"` | |

Key differences:

- **Prepared statements work out of the box** — no `DEALLOCATE` required, transparent caching across connections
- **Multithreaded** — one process, one pool, all CPU cores; no need for `SO_REUSE_PORT` hacks
- **Auto-config** — run `pg_doorman generate --host your-db` to create a config from PostgreSQL
- **Human-readable durations** — `"30s"`, `"5m"`, `"1h"` instead of raw seconds

## patroni_proxy

This repository also includes `patroni_proxy` — a TCP proxy for Patroni-managed PostgreSQL clusters. Zero-downtime failover: existing connections are preserved during cluster topology changes.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="static/patroni_proxy_architecture_dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="static/patroni_proxy_architecture_light.png" />
    <img src="static/patroni_proxy_architecture_light.png" alt="patroni_proxy architecture" width="700" />
  </picture>
</p>

- **pg_doorman** deploys on the same host as PostgreSQL — connection pooling and prepared statement caching benefit from low latency to the database
- **patroni_proxy** deploys as a sidecar in the application pod — TCP routing and role-based failover (leader/sync/async) with least-connections balancing

See [patroni_proxy documentation](https://ozontech.github.io/pg_doorman/tutorials/patroni-proxy.html) for details.

## Documentation

Full documentation, configuration reference, and tutorials: **[ozontech.github.io/pg_doorman](https://ozontech.github.io/pg_doorman/)**

## Contributing

```bash
make pull       # pull test image
make test-bdd   # run all integration tests (Docker-based, fully reproducible)
```

See the [Contributing Guide](https://ozontech.github.io/pg_doorman/tutorials/contributing.html) for details.
