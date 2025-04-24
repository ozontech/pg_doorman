# 1.8.2 (Apr 24th, 2025)

### Added

- `application_name` parameter in pool #30.
- link-time optimization #29.

### Fixed

- If the client does not properly handle errors in copy mode, there may be a risk of connection leakage.


# 1.8.1 (Apr 06th, 2025)

### Fixed

- fix config value of prepared_statements #21.
- close declared cursors #23.
- proxy server parameters #25.

# 1.8.0 (Mar 20th, 2025)

### Fixed

- Dependency fixes #15.

### Added

- release vendor-licenses.txt, [related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com).

# 1.7.9 (Mar 16th, 2025)

### Added

- release vendor.tar.gz for offline build, [related thread](https://www.postgresql.org/message-id/flat/CAMp%2BueYqZNwA5SnZV3-iPOyrmQwnwabyMNMOsu-Rq0sLAa2b0g%40mail.gmail.com).

### Fixed

- This update addresses issues related to the inability to send pqCancel messages over the TLS protocol. 
To clarify, drivers should send pqCancel messages exclusively via TLS if the primary connection was established using TLS.
[Npgsql](https://github.com/npgsql/npgsql) strictly adheres to this rule, however, [PGX](https://github.com/jackc/pgx) currently does not follow this flow, potentially leading to inconsistencies in secure connection handling (aka hostssl).
Both of these behaviors are currently supported and functional.


# 1.7.8 (Mar 8th, 2025)

### Fixed

- In some cases, when using batch processing with the extended protocol, messages were delivered to the client in the wrong order.
- Error messages in the log lacked sufficient detail to diagnose issues encountered during server-side login attempts.

# 1.7.7 (Mar 5th, 2025)

### Added

- `show clients`: added `state` (waiting/idle/active), `wait` (read/write/idle) fields.
- `show servers`: added `state` (login/idle/active), `wait` (read/write/idle), `server_process_pid` fields.
- The proxy timeout for streaming large `message_size_to_be_stream` responses is now set to 15 seconds.

### Fixed

- counter `max_memory_usage` leak when clients disconnect improperly.
