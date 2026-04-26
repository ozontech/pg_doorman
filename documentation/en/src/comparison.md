# PgDoorman vs PgBouncer vs Odyssey vs PgCat

A practical feature matrix for choosing a PostgreSQL connection pooler. PgDoorman targets workloads where prepared statements in transaction mode, multithreaded performance, and operational ergonomics matter.

For benchmark numbers, see [Benchmarks](benchmarks.md).

## Authentication

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| MD5 password | Yes | Yes | Yes |
| SCRAM-SHA-256 (client) | Yes | Yes | Yes |
| SCRAM-SHA-256 passthrough (no plaintext password in config) | Yes | No | Yes |
| MD5 passthrough | Yes | Yes | Yes |
| `auth_query` (dynamic users) | Yes | Yes | Yes |
| `auth_query` passthrough mode (per-user backend identity) | Yes | No | Yes |
| `pg_hba.conf` format | Yes (file or inline) | No | Since 1.4 |
| PAM | Yes (Linux) | Yes (HBA) | Yes |
| JWT (RSA-SHA256) | Yes | No | No |
| Talos (custom JWT with role extraction) | Yes | No | No |
| LDAP | No | Since 1.25 | Yes |
| SCRAM channel binding (`scram-sha-256-plus`) | No | Yes | Yes |
| User name maps (cert/peer → DB user) | No | Since 1.23 | Yes |
| Tunable `scram_iterations` | No | Since 1.25 | No |

See [Authentication](authentication/overview.md).

## TLS

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Client-side TLS (4 modes: disable, allow, require, verify-full) | Yes | Yes | Yes |
| Server-side TLS to PostgreSQL (6 modes incl. verify-ca, verify-full) | Yes | Yes | No |
| mTLS to PostgreSQL (client cert) | Yes | Yes | No |
| Hot reload of TLS certificates on `SIGHUP` | Yes (server-side) | No | No |
| Minimum TLS 1.2 + Mozilla cipher list | Yes | Yes | No (allows TLS 1.0) |
| Direct TLS handshake (PG17, no `SSLRequest`) | No | Since 1.25 | No |
| TLS 1.3 cipher control | No | Since 1.25 | No |

See [TLS](guides/tls.md).

## Routing and high availability

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Patroni-assisted fallback (built-in `/cluster` lookup) | Yes | No | No |
| Bundled TCP proxy with role-based routing (`patroni_proxy`) | Yes | No | No |
| Replica lag guard | Yes (`max_lag_in_bytes` in `patroni_proxy`) | No | Yes (watchdog query) |
| Round-robin / least-connections multi-host | Yes (`patroni_proxy`) | Since 1.24 | Yes |
| `target_session_attrs` (read-write / read-only) | Yes (via `patroni_proxy` roles) | No | Yes |
| Sequential routing (rules in order) | No | No | Yes |
| Connection type routing (TCP vs UNIX) | No | No | Yes |
| Availability zone-aware host selection | No | No | Yes |

See [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md), [`patroni_proxy`](tutorials/patroni-proxy.md).

## Pooling

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Pool modes (transaction, session) | Yes | Yes (+ statement) | Yes |
| Pool Coordinator (cross-user `max_db_connections` with priority eviction) | Yes | No (no eviction) | No |
| Reserve pool with `min_guaranteed_pool_size` | Yes | Reserve only | No |
| Pre-replacement on `server_lifetime` expiry | Yes | No | No |
| Anticipation/burst scaling (`scaling_warm_pool_ratio`, fast retries) | Yes | No | No |
| Direct-handoff (waiter receives returning connection in microseconds) | Yes | No | No |
| `min_pool_size` (warm connections) | Yes | No | Yes |
| Prepared statement cache (two-level, query interner, statement remap) | Yes | Since 1.21 | Since 1.3 |
| Smart `DISCARD` on checkin | RESET ALL + drop cache | No | Yes (auto) |
| LISTEN / NOTIFY pinning in transaction mode | No | No | Experimental |
| Cross-rule connection cap (`shared_pool`) | No | No | Since 1.5.1 |
| `PAUSE` / `RESUME` / `RECONNECT` | Yes | Yes | Yes (1.4.1+) |

See [Pool Coordinator](concepts/pool-coordinator.md), [Pool pressure](tutorials/pool-pressure.md).

## Limits and timeouts

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| `server_idle_check_timeout` (probe before checkout) | Yes | No | No |
| `idle_timeout` (server connection) | Yes | Yes | Yes |
| `server_lifetime` | Yes | Yes | Yes |
| `query_wait_timeout` | Yes | Yes | Yes |
| `client_idle_timeout` | No | Since 1.24 | No |
| `transaction_timeout` | No | Since 1.25 | No |
| `max_user_client_connections` | No | Since 1.24 | No |
| Per-user `query_timeout` | No | Since 1.24 | No |
| Per-user `reserve_pool_size` | No | Since 1.24 | No |
| `query_wait_notify` (NOTICE while waiting for backend) | No | Since 1.25 | Yes (`pool_notice_after_waiting_ms`) |

See [General settings reference](reference/general.md), [Pool settings reference](reference/pool.md).

## Observability

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Built-in Prometheus endpoint | Yes | External (`pgbouncer_exporter`) | Yes |
| Latency percentiles per pool (p50, p90, p95, p99) | Yes (HDR Histogram) | No | Yes (TDigest) |
| Prepared statement counters in stats | Yes | Since 1.24 | No |
| JSON structured logging | Yes (`--log-format Structured`) | No | Yes |
| Runtime log level control (`SET log_level`) | Yes | No | No |
| Admin `SHOW POOL_COORDINATOR` / `SHOW POOL_SCALING` / `SHOW SOCKETS` | Yes | No | No |
| Admin `SHOW PREPARED_STATEMENTS` | Yes | No | No |
| Admin `SHOW HOSTS` (host CPU/memory) | No | No | Yes |
| Admin `SHOW RULES` (dump routing) | No | No | Yes |
| TLS connection metrics (handshake duration, errors, active count) | Yes (server-side) | No | No |
| Patroni API metrics | Yes | No | No |
| Fallback metrics (active flag, current host, hits) | Yes | No | No |

See [Prometheus metrics reference](reference/prometheus.md), [Admin commands](observability/admin-commands.md).

## Operations

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Graceful binary upgrade (zero-downtime, in-flight clients preserved) | Yes | Limited (`SO_REUSEPORT`) | No |
| YAML config | Yes | No (INI) | No (own format) |
| TOML config | Yes (legacy) | No | No |
| Human-readable durations & sizes (`30s`, `1h`, `256MB`) | Yes | No | No |
| Config test mode (`pg_doorman -t`) | Yes | No | No |
| Auto-config from PostgreSQL (`pg_doorman generate --host`) | Yes | No | No |
| `SIGHUP` reload | Yes (incl. server TLS certs) | Yes | Yes |
| systemd `sd-notify` integration | Yes (`Type=notify`) | No | No |
| Memory cap (`max_memory_usage`) | Yes | No | No |

See [Binary upgrade](tutorials/binary-upgrade.md), [Signals](operations/signals.md).

## Protocol

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Simple query | Yes | Yes | Yes |
| Extended query | Yes | Yes | Partial |
| Pipelined batches | Yes | Yes | Partial |
| Async Flush | Yes | Yes | No |
| Cancel requests over TLS | Yes | Yes | Yes |
| `COPY IN` / `COPY OUT` | Yes | Yes | Yes |
| Replication passthrough (`replication=true` startup) | No | Since 1.23 | No |
| Protocol version negotiation (3.2) | No | Since 1.23 | No |
| `server_drop_on_cached_plan_error` | No | No | Since 1.5.1 |

## When PgDoorman is not the right fit

- You need LDAP authentication. Use Odyssey or PgBouncer 1.25+.
- You need SCRAM channel binding (`scram-sha-256-plus`) end-to-end. Use PgBouncer or Odyssey.
- You need replication passthrough for logical replication tools. Use PgBouncer 1.23+.
- You need availability-zone-aware routing or sequential `pg_hba`-style routing rules. Use Odyssey.
- You need `transaction_timeout` enforced by the pooler. Use PgBouncer 1.25+.

For prepared statements in transaction mode, Patroni HA without external proxies, multithreaded throughput, and zero-downtime restarts, PgDoorman is the closer fit.
