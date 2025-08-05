---
title: Changelog
---

# Changelog

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
