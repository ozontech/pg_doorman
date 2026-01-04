# BDD Tests for pg_doorman

This directory contains Behavior-Driven Development (BDD) tests for pg_doorman using Cucumber framework.

## Running Tests

To run all BDD tests:

```bash
cargo test --test bdd
```

## Debug Mode

To enable verbose output during test execution, set the `DEBUG` environment variable:

```bash
DEBUG=1 cargo test --test bdd
```

When DEBUG mode is enabled, the following verbose output will be streamed to the console in real-time:

- **pg_doorman output**: stdout and stderr from the pg_doorman process will be streamed directly to the console
- **PostgreSQL logs**: The PostgreSQL log file (pg.log) will be streamed to the console with `[PG_LOG]` prefix in real-time (tail -f behavior)
- **Shell command output**: Any shell commands executed during tests will have their output streamed immediately (instead of waiting for the streaming threshold)
- **Rust debug logs**: Debug-level tracing logs from the Rust code will be displayed with detailed information including:
  - Target module
  - Thread IDs
  - Line numbers

### Example

```bash
# Run tests with debug output
DEBUG=1 cargo test --test bdd

# Run tests normally (quiet mode)
cargo test --test bdd
```

## Test Structure

- `features/` - Gherkin feature files describing test scenarios
- `main.rs` - Test runner entry point
- `world.rs` - Shared state structure for test scenarios
- `doorman_helper.rs` - Helper functions for starting/stopping pg_doorman
- `postgres_helper.rs` - Helper functions for managing PostgreSQL instances
- `shell_helper.rs` - Helper functions for executing shell commands
- `pg_connection.rs` - PostgreSQL connection utilities
- `extended.rs` - Extended query protocol test steps

## Requirements

- PostgreSQL installed and available in PATH
- Rust toolchain
- Sufficient shared memory available (for PostgreSQL instances)
