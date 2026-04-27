# PgDoorman vs PgBouncer vs Odyssey

Side-by-side feature matrix for choosing a PostgreSQL connection pooler. Every PgBouncer claim is anchored to its [config reference](https://www.pgbouncer.org/config.html) and [changelog](https://www.pgbouncer.org/changelog.html); every Odyssey claim is anchored to the project's [docs](https://github.com/yandex/odyssey/tree/master/docs).

PgCat is intentionally omitted: its design centre is sharding/load-balancing rather than drop-in replacement of PgBouncer, so a row-by-row comparison is misleading. See the [PgCat repo](https://github.com/postgresml/pgcat) if you need horizontal sharding.

For benchmark numbers, see [Benchmarks](benchmarks.md).

## Authentication

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| MD5 password | Yes | Yes | Yes |
| SCRAM-SHA-256 (client → pooler) | Yes | Yes | Yes |
| SCRAM-SHA-256 passthrough (no plaintext password in config) | Yes (`ClientKey` extracted from client proof) | Yes (since 1.14, encrypted SCRAM secret in `auth_query` / `userlist.txt`) | Yes |
| MD5 passthrough | Yes | Yes | Yes |
| `auth_query` (dynamic users) | Yes | Yes | Yes |
| `auth_query` passthrough mode (per-user backend identity) | Yes | No (single `auth_user` for all lookups) | Yes |
| `pg_hba.conf`-style file | Yes (file or inline) | Yes (`auth_hba_file`) | Yes (since 1.4) |
| PAM | Yes (Linux) | Yes (`auth_type=pam` or via HBA) | Yes |
| JWT (RSA-SHA256) | Yes | No | No |
| Talos (custom JWT with role extraction) | Yes (Ozon-specific) | No | No |
| LDAP | No | Yes (since 1.25) | Yes |
| SCRAM channel binding (`scram-sha-256-plus`) | No | Yes | Yes |
| User-name maps (cert/peer → DB user) | No | Yes (since 1.23) | Yes |
| Tunable `scram_iterations` | No | Yes (since 1.25) | No |

See [Authentication](authentication/overview.md).

## TLS

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Client-side TLS (modes: `disable`, `allow`, `require`, `verify-full`) | Yes | Yes (`disable`, `allow`, `prefer`, `require`, `verify-ca`, `verify-full`) | Yes |
| Server-side TLS to PostgreSQL (`disable`, `allow`, `require`, `verify-ca`, `verify-full`) | Yes (5 modes) | Yes (`server_tls_*`, 6 modes incl. `prefer`) | No |
| mTLS to PostgreSQL (client cert sent to backend) | Yes (`server_tls_certificate` + `server_tls_private_key`) | Yes (`server_tls_key_file` + `server_tls_cert_file`) | No |
| Hot reload of server-side TLS certificates | Yes (`SIGHUP`) | Yes (via `RELOAD` / `SIGHUP`, "new file contents will be used for new connections") | No |
| Hot reload of client-facing TLS certificates | No (requires restart or binary upgrade) | Yes (via `RELOAD` / `SIGHUP`) | No |
| Minimum TLS version configurable | Yes (defaults to TLS 1.2) | Yes (`tls_protocols`, default `tlsv1.2,tlsv1.3`) | Configurable, defaults differ |
| Direct TLS handshake (PostgreSQL 17, no `SSLRequest`) | No | Yes (since 1.25) | No |
| TLS 1.3 cipher control | No | Yes (since 1.25, `client_tls13_ciphers`/`server_tls13_ciphers`) | No |
| TLS session migration across binary upgrade | Yes (`tls-migration` build, Linux, opt-in) | No (TLS connections are dropped during online restart) | No |

See [TLS](guides/tls.md).

## Routing and high availability

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Patroni-assisted fallback (built-in `/cluster` lookup) | Yes | No | No |
| Bundled TCP proxy with role-based routing (`patroni_proxy`) | Yes | No | No |
| Replica lag guard | Yes (`max_lag_in_bytes` in `patroni_proxy`) | No | Yes (`watchdog_lag_query` + `catchup_timeout`) |
| Multiple backend hosts with load balancing | Yes (`patroni_proxy`) | Yes (since 1.24, `load_balance_hosts`) | Yes |
| `target_session_attrs` (read-write / read-only routing) | Yes (via `patroni_proxy` roles) | No | Yes |
| Sequential routing rules (first-match wins) | No | No | Yes |
| Connection-type routing (TCP vs UNIX) | No | No | Yes |
| Availability-zone-aware host selection | No | No | Yes |

See [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md), [`patroni_proxy`](tutorials/patroni-proxy.md).

## Pooling

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Pool modes | session, transaction | session, transaction, statement | session, transaction |
| Pool Coordinator (per-database cap with priority eviction) | Yes (`max_db_connections` + p95-ranked eviction) | No (`max_db_connections` queues clients until idle timeout closes existing connections) | No |
| Reserve pool | Yes (`reserve_pool_size`) | Yes (`reserve_pool_size`) | No |
| Per-user `min_guaranteed_pool_size` | Yes | No | No |
| Pre-replacement on `server_lifetime` expiry (warm before old expires) | Yes (95% threshold, up to 3 in parallel) | No | No |
| Anticipation / burst scaling (`scaling_warm_pool_ratio`, fast retries) | Yes | No | No |
| Direct-handoff (returning server goes to longest-waiting client via in-process oneshot channel) | Yes | No | No |
| Strict FIFO ordering of waiters | Yes | No (LIFO via `server_round_robin = 0`) | No |
| `min_pool_size` (warm connections) | Yes | No | Yes |
| Prepared statements in transaction mode | Yes (two-level cache, query interner, `DOORMAN_N` remap) | Yes (since 1.21, `max_prepared_statements`, `PGBOUNCER_*` remap) | Yes (`pool_reserve_prepared_statement`) |
| Smart cleanup on checkin (skip `DEALLOCATE ALL` if cache untouched) | Yes (mutation-tracking `RESET ALL` / `DEALLOCATE ALL` on demand) | No (always `DISCARD ALL` if `server_reset_query` set) | Yes (auto) |
| LISTEN / NOTIFY pinning in transaction mode | No | No | Experimental |
| Cross-rule connection cap (`shared_pool`) | No | No | Yes (since 1.5.1) |
| `PAUSE` / `RESUME` / `RECONNECT` admin commands | Yes | Yes | Yes (since 1.4.1) |

See [Pool Coordinator](concepts/pool-coordinator.md), [Pool pressure](tutorials/pool-pressure.md).

## Limits and timeouts

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| `server_idle_check_timeout` (probe before checkout) | Yes | No | No |
| `idle_timeout` (server-side) | Yes (`idle_timeout`) | Yes (`server_idle_timeout`) | Yes |
| `server_lifetime` | Yes | Yes | Yes |
| `query_wait_timeout` | Yes | Yes | Yes |
| `client_idle_timeout` | No | Yes (since 1.24) | No |
| `transaction_timeout` (pooler-enforced) | No | Yes (since 1.25) | No |
| `max_user_client_connections` | No | Yes (since 1.24) | No |
| `max_db_client_connections` | No | Yes (since 1.24) | No |
| Per-user `query_timeout` | No | Yes (since 1.24) | No |
| Per-user `reserve_pool_size` | No | Yes (since 1.24) | No |
| Notify client while waiting for backend | No | Yes (since 1.25, `query_wait_notify`) | Yes (`pool_notice_after_waiting_ms`) |

See [General settings reference](reference/general.md), [Pool settings reference](reference/pool.md).

## Observability

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Prometheus endpoint | Built-in `/metrics` | External (`pgbouncer_exporter`) | External (Go exporter sidecar that polls the admin console) |
| Latency percentiles per pool (p50, p90, p95, p99) | Yes (HDR Histogram) | No (averages only in `SHOW STATS`) | Yes via the exporter (TDigest, requires `quantiles` rule option) |
| Prepared statement counters in `SHOW STATS` | Yes | Yes (since 1.24) | No |
| JSON structured logging | Yes (`--log-format structured`) | No | Yes (`log_format "json"`) |
| Runtime log level control (`SET log_level`) | Yes | No | No |
| `SHOW POOL_COORDINATOR` / `SHOW POOL_SCALING` / `SHOW SOCKETS` | Yes | No | No |
| `SHOW PREPARED_STATEMENTS` | Yes | No | No |
| `SHOW HOSTS` (host CPU/memory) | No | No | Yes |
| `SHOW RULES` (dump effective routing) | No | No | Yes |
| Server-side TLS connection metrics (handshake duration, errors, active count) | Yes | No | No |
| Patroni API metrics | Yes | No | No |
| Fallback metrics (active flag, current host, hits) | Yes | No | No |

See [Prometheus metrics reference](reference/prometheus.md), [Admin commands](observability/admin-commands.md).

## Operations

| Feature | PgDoorman | PgBouncer | Odyssey |
| --- | :-: | :-: | :-: |
| Binary upgrade with session migration (TCP socket, cancel keys, prepared cache) | Yes (`SCM_RIGHTS`, plus TLS state with the `tls-migration` build) | No: `-R` deprecated since 1.20; `so_reuseport` rolling restart drains old sessions in place | No: `SIGUSR2` + `bindwith_reuseport` drains old sessions in place |
| Configuration format | YAML or TOML | INI | Own format (lex/yacc) |
| Human-readable durations and sizes (`30s`, `1h`, `256MB`) | Yes | No (integer microseconds / bytes) | No |
| Config test mode (`pg_doorman -t`) | Yes | No | No |
| Auto-config from PostgreSQL (`pg_doorman generate --host`) | Yes | No | No |
| `SIGHUP` reload | Yes (server TLS certs included; client TLS still requires restart) | Yes (`auth_file`, `auth_hba_file`, server and client TLS certs) | Yes |
| systemd `sd-notify` (`Type=notify`) integration | Yes | No | No |
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
| Replication passthrough (`replication=true` startup) | No | Yes (since 1.23) | No |
| Protocol version negotiation (3.2) | No | Yes (since 1.23) | No |
| `server_drop_on_cached_plan_error` | No | No | Yes (since 1.5.1) |

## When PgDoorman is not the right fit

- **You need LDAP authentication.** Use Odyssey or PgBouncer 1.25+.
- **You need replication passthrough for logical replication tools.** Use PgBouncer 1.23+.
- **You need `transaction_timeout` enforced by the pooler.** Use PgBouncer 1.25+.
- **You need horizontal sharding inside the pooler.** Use PgCat.

For prepared statements in transaction mode, Patroni HA without external proxies, multi-threaded throughput in a single shared pool, and binary upgrades that migrate live sessions, PgDoorman is the closer fit.
