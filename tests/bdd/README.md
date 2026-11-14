# BDD Tests for pg_doorman

This directory contains Behavior-Driven Development (BDD) tests for pg_doorman using Cucumber framework.

## Running Tests

To run all BDD tests:

```bash
cargo test --test bdd
```

## Backend batch regression

Run the backend batch scenarios in the Linux test container:

```bash
make test-bdd TAGS=@backend-batch
```

They check a large request/response exchange, a paced 64-query batch directly
against PostgreSQL, and the same paced batch through pg_doorman. The client
drains replies immediately. Small backend socket buffers and `pg_sleep(0.150)`
make the transfer exceed the former five-second send deadline despite replies
arriving throughout the batch.

The `@backend-batch-progress` scenario asserts successful completion beyond
five seconds. It previously failed with `58006` while rows were still arriving.
The other two scenarios are controls. This regression is included in `@rust` /
`@rust-2`. The `@backend-write-stall` scenarios also check recovery after freezing
PostgreSQL for six seconds, for both an extended batch and a simple query.

```bash
# Only the paced batch regression:
make test-bdd TAGS=@backend-batch-progress
# A paused backend resumes without losing the session:
make test-bdd TAGS=@backend-write-stall
# Only the large request/response exchange:
make test-bdd TAGS=@backend-duplex-large-batch
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

## Benchmarks

Benchmarks are located in `tests/bdd/features/bench.feature` and can be run using the `@bench` tag:

```bash
cargo test --test bdd -- --tags @bench
```

### Parameterization

You can parameterize benchmarks using environment variables:

- `BENCH_DOORMAN_WORKERS`: Number of worker threads for `pg_doorman` (default: 12)
- `BENCH_ODYSSEY_WORKERS`: Number of workers for `odyssey` (default: 12)
- `BENCH_PGBENCH_JOBS`: Global number of threads (`-j`) for `pgbench`. If set, it overrides all specific job settings.
- `BENCH_PGBENCH_JOBS_C1`: Number of threads for 1-client tests (default: 1)
- `BENCH_PGBENCH_JOBS_C40`: Number of threads for 40-client tests (default: 4)
- `BENCH_PGBENCH_JOBS_C120`: Number of threads for 120-client tests (default: 4)
- `BENCH_PGBENCH_JOBS_C500`: Number of threads for 500-client tests (default: 4)
- `BENCH_PGBENCH_JOBS_C10000`: Number of threads for 10,000-client tests (default: 4)
- `FARGATE_CPU`: AWS Fargate CPU units (optional, for reporting)
- `FARGATE_MEMORY`: AWS Fargate memory in MB (optional, for reporting)

## Requirements

- PostgreSQL installed and available in PATH
- Rust toolchain
- Sufficient shared memory available (for PostgreSQL instances)
