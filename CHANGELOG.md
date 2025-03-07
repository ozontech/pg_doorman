# 1.7.8 (Mar 8th, 2025)

### Fixed

- In some cases, when batch processing and extended protocol were used, messages arrived to the client in the wrong order.
- Error messages in the log contained insufficient information to understand exactly what happened when trying to log in to the server side.

# 1.7.7 (Mar 5th, 2025)

### Added

- `show clients`: added `state` (waiting/idle/active), `wait` (read/write/idle).
- `show servers`: added `state` (login/idle/active), `wait` (read/write/idle), `server_process_pid`.
- proxy timeout for streaming big `message_size_to_be_stream` responses is 15s.

### Fixed

- counter `max_memory_usage` leak while client is disconnecting incorrectly.
