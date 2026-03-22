# Changelog

## [Unreleased]

### Fixed

- **Session mode: keep server connections alive after SQL errors.** A query like `SELECT 1/0` returns an `ErrorResponse` from PostgreSQL but leaves the connection fully usable. Previously, `handle_error_response` called `mark_bad()` unconditionally in async mode, so the connection was destroyed at session end. Now `mark_bad` is skipped when the pool runs in session mode. Transaction mode still calls `mark_bad` because the connection returns to a shared pool where protocol desync is dangerous.
