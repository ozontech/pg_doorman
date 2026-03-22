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

**Full extended query protocol support.** Benchmarks show Odyssey is up to 61% slower with the extended query protocol in transaction mode. PgDoorman handles simple, extended, and prepared protocols equally well — including pipelined batches and async Flush flow that cause issues in other poolers.

**Lazy server acquisition.** PgDoorman defers backend connection allocation until the first real query inside a transaction. A standalone `BEGIN` does not grab a server connection. If a client opens a transaction and disconnects without sending a query, no backend connection is used at all. PgBouncer and Odyssey allocate a server connection on `BEGIN`.

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
| Auth query passthrough (per-user backend identity) | Yes | No | No |
| PAM auth | Yes | Yes | Yes |
| LDAP auth | No | Since 1.25 | Yes |
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

## Stale Backend Detection

When a client holds a transaction open but sends no queries, the backend connection can die silently (failover, OOM kill, network partition). With other poolers the client hangs until TCP keepalive fires — typically 2+ minutes.

PgDoorman probes the backend when a client is idle in transaction. If the backend is gone, the client gets an error immediately. Controlled by `idle_client_in_transaction_timeout`.

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

Encrypt connections from PgDoorman to PostgreSQL:

```yaml
general:
  server_tls: true
  verify_server_certificate: true         # verify PostgreSQL server certificate
```

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
| `pg_doorman_pool_prepared_cache_entries` | user, database | Prepared statement cache entries            |
| `pg_doorman_auth_query_cache` | event, database | Auth query cache hits / misses              |
| `pg_doorman_auth_query_dynamic_pools` | database | Active dynamic user pools                    |
| `pg_doorman_total_memory` | — | Process memory usage (bytes)                |
| `pg_doorman_connection_count` | type | Connections by type (plain / tls / total)   |

## Signals & Zero-Downtime Upgrade

| Signal | Effect |
|--------|--------|
| `SIGHUP` | Reload configuration without restart |
| `SIGUSR2` | Start binary upgrade + graceful shutdown of old process |
| `SIGTERM` | Immediate shutdown |

### Binary upgrade (zero downtime)

Replace the pg_doorman binary while clients stay connected:

```bash
# Replace the binary on disk, then:
kill -USR2 $(cat /tmp/pg_doorman.pid)

# Or from the admin console:
UPGRADE;
```

PgDoorman validates the new binary's configuration (`-t` flag) before starting it. If validation fails, the upgrade is aborted and the old process continues. Active clients experience no interruption — new connections are served by the new process, existing ones drain gracefully.

For systemd services:

```ini
ExecReload=/bin/kill -SIGUSR2 $MAINPID
```

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
