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
