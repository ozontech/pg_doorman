---
title: Changelog
---

# Changelog

## PgDoorman

### 1.8.2 <small>May 24, 2025</small> { id="1.8.2" }

- Added `application_name` parameter in pool #30.
- Added link-time optimization #29.
- Added `DISCARD ALL`/`DEALLOCATE ALL` client query support.
- Fixed panics in admin console.
- Fixed connection leakage on inproperly handled errors in client's copy mode

### 1.8.1 <small>April 12, 2025</small> { id="1.8.1" }

- Fixed config value of prepared_statements #21.
- Fixed close declared cursors #23.
- Fixed proxy server parameters #25.

### 1.8.0 <small>Mar 20, 2025</small> { id="1.8.0" }

- Fixed #15: Dependencies
- Added release vendor-licenses.txt, [related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com).

### 1.7.9 <small>Mar 16, 2025</small> { id="1.7.9" }

- Added release vendor.tar.gz for offline build, [related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com).

- Fixed This update addresses issues related to the inability to send pqCancel messages over the TLS protocol. 
To clarify, drivers should send pqCancel messages exclusively via TLS if the primary connection was established using TLS.
[Npgsql](https://github.com/npgsql/npgsql) strictly adheres to this rule, however, [PGX](https://github.com/jackc/pgx) currently does not follow this flow, potentially leading to inconsistencies in secure connection handling (aka hostssl).
Both of these behaviors are currently supported and functional.

### 1.7.8 <small>Mar 8, 2025</small> { id="1.7.8" }

- Fixed In some cases, when using batch processing with the extended protocol, messages were delivered to the client in the wrong order.
- Fixed Error messages in the log lacked sufficient detail to diagnose issues encountered during server-side login attempts.

### 1.7.7 <small>Mar 8, 2025</small> { id="1.7.7" }

- Added `show clients`: added `state` (waiting/idle/active), `wait` (read/write/idle) fields.
- Added `show servers`: added `state` (login/idle/active), `wait` (read/write/idle), `server_process_pid` fields.
- Added The proxy timeout for streaming large `message_size_to_be_stream` responses is now set to 15 seconds.
- Fixed counter `max_memory_usage` leak when clients disconnect improperly.
