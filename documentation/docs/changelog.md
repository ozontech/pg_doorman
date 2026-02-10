---
title: Changelog
---

# Changelog

### 3.2.3 <small>Feb 10, 2026</small> { id="3.2.3" }

**Improvements:**

- **Jitter for `server_lifetime` (±20%)**: Connection lifetimes now have a random ±20% jitter applied to prevent mass disconnections from PostgreSQL. When pg_doorman is under heavy load, it creates many connections simultaneously, which previously caused them all to expire at the same time, creating spikes of connection closures. Now each connection gets an individual lifetime calculated as `base_lifetime ± random(20%)`. For example, with `server_lifetime: 300000` (5 minutes), actual lifetimes range from 240s to 360s, spreading connection closures evenly over time.

### 3.2.2 <small>Feb 9, 2026</small> { id="3.2.2" }

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

### 3.1.8 <small>Jan 31, 2026</small> { id="3.1.8" }

**Bug Fixes:**

- **Fixed ParseComplete desynchronization in pipeline on errors**: Fixed a protocol desynchronization issue (especially noticeable in .NET Npgsql driver) where synthetic `ParseComplete` messages were not being inserted if an error occurred during a pipelined batch. When the pooler caches a prepared statement and skips sending `Parse` to the server, it must still provide a `ParseComplete` to the client. If an error occurs before subsequent commands are processed, the server skips them, and the pooler now ensures all missing synthetic `ParseComplete` messages are inserted into the response stream upon receiving an `ErrorResponse` or `ReadyForQuery`.

- **Fixed incorrect `use_savepoint` state persistence**: Fixed a bug where the `use_savepoint` flag (which disables automatic rollback on connection return if a savepoint was used) was not reset after a transaction ended.


### 3.1.7 <small>Jan 28, 2026</small> { id="3.1.7" }

**Memory Optimization:**

- **DEALLOCATE now clears client prepared statements cache**: When a client sends `DEALLOCATE <name>` or `DEALLOCATE ALL` via simple query protocol, the pooler now properly clears the corresponding entries from the client's internal prepared statements cache. Previously, synthetic OK responses were sent but the client cache was not cleared, causing memory to grow indefinitely for long-running connections using many unique prepared statements. This fix allows memory to be reclaimed when clients properly deallocate their statements.

- **New `client_prepared_statements_cache_size` configuration parameter**: Added protection against malicious or misbehaving clients that don't call `DEALLOCATE` and could exhaust server memory by creating unlimited prepared statements. When the per-client cache limit is reached, the oldest entry is evicted automatically. Set to `0` for unlimited (default, relies on client calling `DEALLOCATE`). Example: `client_prepared_statements_cache_size: 1024` limits each client to 1024 cached prepared statements.

### 3.1.6 <small>Jan 27, 2026</small> { id="3.1.6" }

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

### 3.1.5 <small>Jan 25, 2026</small> { id="3.1.5" }

**Bug Fixes:**

- **Fixed PROTOCOL VIOLATION with batch PrepareAsync**
- **Rewritten ParseComplete insertion algorithm**

**Performance:**

- **Deferred connection acquisition for standalone BEGIN**: When a client sends a standalone `BEGIN;` or `begin;` query (simple query protocol), the pooler now defers acquiring a server connection until the next message arrives. Since `BEGIN` itself doesn't perform any actual database operations, this optimization reduces connection pool contention when clients are slow to send their next query after starting a transaction.
  - Micro-optimized detection: first checks message size (12 bytes), then content using case-insensitive comparison
  - If client sends Terminate (`X`) after `BEGIN`, no server connection is acquired at all
  - The deferred `BEGIN` is automatically sent to the server before the actual query

### 3.1.0 <small>Jan 18, 2026</small> { id="3.1.0" }

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

### 3.0.5 <small>Jan 16, 2026</small> { id="3.0.5" }

**Bug Fixes:**

- Fixed panic (`capacity overflow`) in startup message handling when receiving malformed messages with invalid length (less than 8 bytes or exceeding 10MB). Now gracefully rejects such connections with `ClientBadStartup` error.

**Testing:**

- **Integration fuzz testing framework**: Added comprehensive BDD-based fuzz tests (`@fuzz` tag) that verify pg_doorman's resilience to malformed PostgreSQL protocol messages.
- All fuzz tests connect and authenticate first, then send malformed data to test post-authentication resilience.

**CI/CD:**

- Added dedicated fuzz test job in GitHub Actions workflow (without retries, as fuzz tests should not be flaky).

### 3.0.4 <small>Jan 16, 2026</small> { id="3.0.4" }

**New Features:**

- **Enhanced DEBUG logging for PostgreSQL protocol messages**: Added grouped debug logging that displays message types in a compact format (e.g., `[P(stmt1),B,D,E,S]` or `[3xD,C,Z]`). Messages are buffered and flushed every 100ms or 100 messages to reduce log noise.
- **Protocol violation detection**: Added real-time protocol state tracking that detects and warns about protocol violations (e.g., receiving ParseComplete when no Parse was pending). Helps diagnose client-server synchronization issues.

**Bug Fixes:**

- Fixed potential protocol violation when client disconnects during batch operations with cached prepared statements: disabled fast_release optimization when there are pending prepared statement operations.
- Fixed ParseComplete insertion for Describe flow: now correctly inserts one ParseComplete before each ParameterDescription ('t') or NoData ('n') message instead of inserting all at once.

### 3.0.3 <small>Jan 15, 2026</small> { id="3.0.3" }

**Bug Fixes:**

- Improved handling of Describe flow for cached prepared statements: added a separate counter (`pending_parse_complete_for_describe`) to correctly insert ParseComplete messages before ParameterDescription or NoData responses when Parse was skipped due to caching.

**Testing:**

- Added comprehensive .NET client tests for Describe flow with cached prepared statements (`describe_flow_cached.cs`).
- Added aggressive mixed tests combining batch operations, prepared statements, and extended protocol (`aggressive_mixed.cs`).

### 3.0.2 <small>Jan 14, 2026</small> { id="3.0.2" }

**Bug Fixes:**

- Fixed protocol mismatch for .NET clients (Npgsql) using named prepared statements with `Prepare()`: ParseComplete messages are now correctly inserted before ParameterDescription and NoData messages in the Describe flow, not just before BindComplete.

### 3.0.1 <small>Jan 14, 2026</small> { id="3.0.1" }

**Bug Fixes:**

- Fixed protocol mismatch for .NET clients (Npgsql): prevented insertion of ParseComplete messages between DataRow messages when server has more data available.

**Testing:**

- Extended Node.js client test coverage with additional scenarios for prepared statements, error handling, transactions, and edge cases.

### 3.0.0 <small>Jan 12, 2026</small> { id="3.0.0" }

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

### 2.5.0 <small>Nov 18, 2025</small> { id="2.5.0" }

**Improvements:**
- Reworked the statistics collection system, yielding up to 20% performance gain on fast queries.
- Improved detection of `SAVEPOINT` usage, allowing the auto-rollback feature to be applied in more situations.

**Bug Fixes / Behavior:**
- Less aggressive behavior on write errors when sending a response to the client: the server connection is no longer immediately marked as "bad" and evicted from the pool. We now read the remaining server response and clean up its state, returning the connection to the pool in a clean state. This improves performance during client reconnections.


### 2.4.3 <small>Nov 15, 2025</small> { id="2.4.3" }

**Bug Fixes:**
- Fixed handling of nested transactions via `SAVEPOINT`: auto-rollback now correctly rolls back to the savepoint instead of breaking the outer transaction. This prevents clients from getting stuck in an inconsistent transactional state.


### 2.4.2 <small>Nov 13, 2025</small> { id="2.4.2" }

**Improvements:**
- `pg_hba` rules now apply to the admin console as well; the `trust` method can be used for admin connections when a matching rule is present (use with caution; restrict by address/TLS).

**Bug Fixes:**
- Fixed `pg_hba` evaluation: `local` records were mistakenly considered; PgDoorman only handles TCP connections, so `local` entries are now correctly ignored.



### 2.4.1 <small>Nov 12, 2025</small> { id="2.4.1" }

**Improvements:**
- Performance optimizations in request handling and message processing paths to reduce latency and CPU usage.
- `pg_hba` rules now apply to the admin console as well; the `trust` method can be used for admin connections when a matching rule is present (use with caution; restrict by address/TLS).

**Bug Fixes:**
- Corrected logic where `COMMIT` could be mishandled similarly to `ROLLBACK` in certain error states; now transactional state handling is aligned with PostgreSQL semantics.


### 2.4.0 <small>Nov 10, 2025</small> { id="2.4.0" }

**Features:**
- Added `pg_hba` support to control client access in PostgreSQL format. New `general.pg_hba` setting supports inline content or file path.
- Clients that enter the `aborted in transaction` state are detached from their server backend; the proxy waits for the client to send `ROLLBACK`.

**Improvements:**
- Refined admin and metrics counters: separated `cancel` connections and corrected calculation of `error` connections in admin output and Prometheus metrics descriptions.
- Added configuration validation to prevent simultaneous use of legacy `general.hba` CIDR list with the new `general.pg_hba` rules.
- Improved validation and error messages for Talos token authentication.

### 2.2.2 <small>Aug 17, 2025</small> { id="2.2.2" }

**Features:**
- Added new generate feature functionality

**Bug Fixes:**
- Fixed deallocate issues with PGX5 compatibility

### 2.2.1 <small>Aug 6, 2025</small> { id="2.2.1" }

**Features:**
- Improve Prometheus exporter functionality

### 2.2.0 <small>Aug 5, 2025</small> { id="2.2.0" }

**Features:**
- Added Prometheus exporter functionality that provides metrics about connections, memory usage, pools, queries, and transactions

### 2.1.2 <small>Aug 4, 2025</small> { id="2.1.2" }

**Features:**
- Added docker image `ghcr.io/ozontech/pg_doorman`


### 2.1.0 <small>Aug 1, 2025</small> { id="2.1.0" }

**Features:**
- The new command `generate` connects to your PostgreSQL server, automatically detects all databases and users, and creates a complete configuration file with appropriate settings. This is especially useful for quickly setting up PgDoorman in new environments or when you have many databases and users to configure.


### 2.0.1 <small>July 24, 2025</small> { id="2.0.1" }

**Bug Fixes:**
- Fixed `max_memory_usage` counter leak when clients disconnect improperly.

### 2.0.0 <small>July 22, 2025</small> { id="2.0.0" }

**Features:**
- Added `tls_mode` configuration option to enhance security with flexible TLS connection management and client certificate validation capabilities.

### 1.9.0 <small>July 20, 2025</small> { id="1.9.0" }

**Features:**
- Added PAM authentication support.
- Added `talos` JWT authentication support.

**Improvements:**
- Implemented streaming for COPY protocol with large columns to prevent memory exhaustion.
- Updated Rust and Tokio dependencies.

### 1.8.3 <small>Jun 11, 2025</small> { id="1.8.3" }

**Bug Fixes:**
- Fixed critical bug where Client's buffer wasn't cleared when no free connections were available in the Server pool (query_wait_timeout), leading to incorrect response errors. [#38](https://github.com/ozontech/pg_doorman/pull/38)
- Fixed Npgsql-related issue. [Npgsql#6115](https://github.com/npgsql/npgsql/issues/6115)

### 1.8.2 <small>May 24, 2025</small> { id="1.8.2" }

**Features:**
- Added `application_name` parameter in pool. [#30](https://github.com/ozontech/pg_doorman/pull/30)
- Added support for `DISCARD ALL` and `DEALLOCATE ALL` client queries.

**Improvements:**
- Implemented link-time optimization. [#29](https://github.com/ozontech/pg_doorman/pull/29)

**Bug Fixes:**
- Fixed panics in admin console.
- Fixed connection leakage on improperly handled errors in client's copy mode.

### 1.8.1 <small>April 12, 2025</small> { id="1.8.1" }

**Bug Fixes:**
- Fixed config value of prepared_statements. [#21](https://github.com/ozontech/pg_doorman/pull/21)
- Fixed handling of declared cursors closure. [#23](https://github.com/ozontech/pg_doorman/pull/23)
- Fixed proxy server parameters. [#25](https://github.com/ozontech/pg_doorman/pull/25)

### 1.8.0 <small>Mar 20, 2025</small> { id="1.8.0" }

**Bug Fixes:**
- Fixed dependencies issue. [#15](https://github.com/ozontech/pg_doorman/pull/15)

**Improvements:**
- Added release vendor-licenses.txt file. [Related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com)

### 1.7.9 <small>Mar 16, 2025</small> { id="1.7.9" }

**Improvements:**
- Added release vendor.tar.gz for offline build. [Related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com)

**Bug Fixes:**
- Fixed issues with pqCancel messages over TLS protocol. Drivers should send pqCancel messages exclusively via TLS if the primary connection was established using TLS. [Npgsql](https://github.com/npgsql/npgsql) follows this rule, while [PGX](https://github.com/jackc/pgx) currently does not. Both behaviors are now supported.

### 1.7.8 <small>Mar 8, 2025</small> { id="1.7.8" }

**Bug Fixes:**
- Fixed message ordering issue when using batch processing with the extended protocol.
- Improved error message detail in logs for server-side login attempt failures.

### 1.7.7 <small>Mar 8, 2025</small> { id="1.7.7" }

**Features:**
- Enhanced `show clients` command with new fields: `state` (waiting/idle/active) and `wait` (read/write/idle).
- Enhanced `show servers` command with new fields: `state` (login/idle/active), `wait` (read/write/idle), and `server_process_pid`.
- Added 15-second proxy timeout for streaming large `message_size_to_be_stream` responses.

**Bug Fixes:**
- Fixed `max_memory_usage` counter leak when clients disconnect improperly.
