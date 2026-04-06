# Changelog

### 3.4.0 <small>Apr 1, 2026</small>

**New Features:**

- **Pool Coordinator — database-level connection limits.** New `max_db_connections` setting caps total server connections per database across all user pools. When the limit is reached, the coordinator evicts idle connections from users with the largest surplus (respecting `min_guaranteed_pool_size`), then waits for a connection to be returned, and falls back to a reserve pool as last resort. Disabled by default (`max_db_connections = 0`) — zero overhead when not configured. Five new pool-level config fields: `max_db_connections`, `min_connection_lifetime` (eviction age threshold), `reserve_pool_size` (extra slots beyond the limit), `reserve_pool_timeout` (wait before using reserve), `min_guaranteed_pool_size` (per-user eviction protection independent of `min_pool_size`).

- **`SHOW POOL_COORDINATOR` admin command.** Displays per-database coordinator status: configured limits, current connection count, reserve usage, cumulative evictions, reserve acquisitions, and client exhaustion errors.

- **Pool Coordinator Prometheus metrics.** Seven new metrics under `pg_doorman_pool_coordinator{type, database}`: `connections` (current), `reserve_in_use` (current), `max_connections` (configured limit), `reserve_pool_size` (configured reserve), `evictions_total`, `reserve_acquisitions_total`, `exhaustions_total` (client errors from full exhaustion — primary pager signal).

- **Reserve pressure relief.** Idle reserve connections (created under `max_db_connections` pressure) are closed early by the retain cycle once idle longer than `min_connection_lifetime`, returning reserve capacity before the regular `idle_timeout` fires.

- **Runtime log level control via admin `SET` command.** Change log level without restarting the pooler: `SET log_level = 'debug'` for global, `SET log_level = 'warn,pg_doorman::pool::pool_coordinator=debug'` for per-module (RUST_LOG syntax). View current level with `SHOW LOG_LEVEL`. Changes are ephemeral (lost on restart). Zero overhead on the hot path at production log levels — filtering uses lock-free `ArcSwap` instead of `RwLock`.

- **Anticipation + bounded burst for connection creation.** Replaces the per-task blind cooldown sleep with two coordinated mechanisms that suppress thundering-herd backend connects under load. (1) Returns to the idle pool now signal a `tokio::sync::Notify`, so callers waiting in the cooldown zone wake event-driven and exactly one waiter is woken per return — no more N-task polling races after a `sleep(10ms)`. (2) Concurrent server creates per pool are capped by an atomic burst gate; overflow callers wait for either an idle return or a peer create completion before retrying recycle. The legacy `scaling_cooldown_sleep` knob is replaced by `scaling_max_anticipation_wait_ms` (default `100ms`, the upper bound on the event-driven idle wait) and `scaling_max_parallel_creates` (default `2`, the per-pool create cap). Both are global; per-pool overrides for these are intentionally not supported. The retain-loop replenish path also respects the burst cap so background replenishment does not compete with client-driven creates during a load spike. Hot-path overhead is ~3 ns on the burst gate atomic and ~104 ns on a buffered notify wake.

**Improvements:**

- **Log readability overhaul.** All operational log messages use a consistent `[user@pool]` identity prefix with `pid=N` for server connections, replacing three different formats. Stats line switched to logfmt (`query_ms p50=... | xact_ms p50=...`). Durations formatted as `4m30s` instead of raw milliseconds or `0d 00:04:30.134`. PG error messages sanitized (newlines escaped) to prevent log line splitting.

- **Auth failure logs include client IP.** SCRAM, MD5, JWT, and PAM authentication failures now show the source address, visible during brute-force attempts.

- **Replenish failure noise suppression.** When `min_pool_size` replenish repeatedly fails (e.g., SCRAM user missing on a replica), only the first failure is logged at warn level. Subsequent failures are suppressed with a periodic reminder every ~10 minutes showing the consecutive failure count. Recovery is logged when replenish succeeds after a failure streak.

- **Client disconnect logs include identity.** Disconnect messages show `[user@pool]` when the client completed authentication, not just the IP address.

- **Prepared statement cache eviction log.** Shows truncated query text and current cache size (`size=99/100`) to help diagnose cache sizing issues.

**Security:**

- **Removed password hash from logs.** The "unsupported password type" warning no longer includes the password hash value.

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

- **`idle_timeout = 0` did not disable idle timeout**: Setting `idle_timeout` to `0` was supposed to disable idle connection cleanup (consistent with PgBouncer's `server_idle_timeout = 0` semantics and our own `server_lifetime = 0` behavior). Instead, it closed connections after ~1ms of being idle. Fixed by adding an `idle_timeout_ms > 0` guard before the elapsed time check.

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

- **Integration fuzz testing framework**: Added comprehensive BDD-based fuzz tests (`@fuzz` tag) that verify pg_doorman's resilience to malformed PostgreSQL protocol messages.
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

- Added comprehensive .NET client tests for Describe flow with cached prepared statements (`describe_flow_cached.cs`).
- Added aggressive mixed tests combining batch operations, prepared statements, and extended protocol (`aggressive_mixed.cs`).

### 3.0.2 <small>Jan 14, 2026</small>

**Bug Fixes:**

- Fixed protocol mismatch for .NET clients (Npgsql) using named prepared statements with `Prepare()`: ParseComplete messages are now correctly inserted before ParameterDescription and NoData messages in the Describe flow, not just before BindComplete.

### 3.0.1 <small>Jan 14, 2026</small>

**Bug Fixes:**

- Fixed protocol mismatch for .NET clients (Npgsql): prevented insertion of ParseComplete messages between DataRow messages when server has more data available.

**Testing:**

- Extended Node.js client test coverage with additional scenarios for prepared statements, error handling, transactions, and edge cases.

### 3.0.0 <small>Jan 12, 2026</small>

**Major Release — Complete Architecture Refactoring**

This release represents a significant milestone with a complete codebase refactoring that dramatically improves async protocol support, making PgDoorman the most efficient connection pooler for asynchronous PostgreSQL workloads.

**New Features:**

- **patroni_proxy** — A new high-performance TCP proxy for Patroni-managed PostgreSQL clusters:
    - Zero-downtime connection management — existing connections are preserved during cluster topology changes
    - Hot upstream updates — automatic discovery of cluster members via Patroni REST API without connection drops
    - Role-based routing — route connections to leader, sync replicas, or async replicas based on configuration
    - Replication lag awareness with configurable `max_lag_in_bytes` per port
    - Least connections load balancing strategy

**Improvements:**

- **Complete codebase refactoring** — modular architecture with better separation of concerns:
    - Client handling split into dedicated modules (core, entrypoint, protocol, startup, transaction)
    - Configuration system reorganized into focused modules (general, pool, user, tls, prometheus, talos)
    - Admin, auth, and prometheus subsystems extracted into separate modules
    - Improved code maintainability and testability
- **Enhanced async protocol support** — significantly improved handling of asynchronous PostgreSQL protocol, providing better performance than other connection poolers for async workloads
- **Extended protocol improvements** — better client buffering and message handling for extended query protocol
- **xxhash3 for prepared statement hashing** — faster hash computation for prepared statement cache
- **Comprehensive BDD testing framework** — multi-language integration tests (Go, Rust, Python, Node.js, .NET) with Docker-based reproducible environment

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
