---
title: Changelog
---

# Changelog

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
