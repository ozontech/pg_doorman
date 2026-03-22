# Changelog

## [Unreleased]

### Fixed

- **Session mode: stop destroying healthy connections on SQL errors.**
  In session mode, a PostgreSQL `ErrorResponse` during async operation (e.g. syntax error, constraint violation) no longer marks the server connection as bad. The connection stays in the pool and continues serving the client. Transaction mode behavior is unchanged — `mark_bad` is still called there because the connection may have desynchronized protocol state.
