---
title: Contributing to PgDoorman
---

# Contributing to PgDoorman

Thank you for your interest in contributing to PgDoorman! This guide will help you set up your development environment and understand the contribution process.

## Getting Started

### Prerequisites

For running integration tests, you only need:

- [Docker](https://docs.docker.com/get-docker/) (required)
- [Make](https://www.gnu.org/software/make/) (required)

**Nix installation is NOT required** — test environment reproducibility is ensured by Docker containers built with Nix.

For local development (optional):
- [Rust](https://www.rust-lang.org/tools/install) (latest stable version)
- [Git](https://git-scm.com/downloads)

### Setting Up Your Development Environment

1. **Fork the repository** on GitHub
2. **Clone your fork**:
   ```bash
   git clone https://github.com/YOUR-USERNAME/pg_doorman.git
   cd pg_doorman
   ```
3. **Add the upstream repository**:
   ```bash
   git remote add upstream https://github.com/ozontech/pg_doorman.git
   ```

## Local Development

1. **Build the project**:
   ```bash
   cargo build
   ```

2. **Build for performance testing**:
   ```bash
   cargo build --release
   ```

3. **Configure PgDoorman**:
   - Copy the example configuration: `cp pg_doorman.toml.example pg_doorman.toml`
   - Adjust the configuration in `pg_doorman.toml` to match your setup

4. **Run PgDoorman**:
   ```bash
   cargo run --release
   ```

5. **Run unit tests**:
   ```bash
   cargo test
   ```

## Integration Testing

PgDoorman uses BDD (Behavior-Driven Development) tests with a Docker-based test environment. **Reproducibility is guaranteed** — all tests run inside Docker containers with identical environments.

### Test Environment

The test Docker image (built with Nix) includes:
- PostgreSQL 16
- Go 1.24
- Python 3 with asyncpg, psycopg2, aiopg, pytest
- Node.js 22
- .NET SDK 8
- Rust 1.87.0

### Running Tests

From the **project root directory**:

```bash
# Pull the test image from registry
make pull

# Or build locally (takes 10-15 minutes on first run)
make local-build

# Run all BDD tests
make test-bdd

# Run tests with specific tag
make test-bdd TAGS=@copy-protocol
make test-bdd TAGS=@cancel
make test-bdd TAGS=@admin-commands

# Open interactive shell in test container
make shell
```

### Debug Mode

Enable debug output with the `DEBUG=1` environment variable:

```bash
DEBUG=1 make test-bdd TAGS=@copy-protocol
```

When `DEBUG=1` is set:
- Tracing is enabled with DEBUG level
- Thread IDs are shown in logs
- Line numbers are included
- PostgreSQL protocol details are visible
- Detailed step-by-step execution is logged

This is useful when:
- Debugging failing tests
- Understanding protocol-level communication
- Investigating timing issues
- Developing new test scenarios

### Available Test Tags

| Tag | Description |
|-----|-------------|
| `@go` | Go client tests |
| `@python` | Python client tests |
| `@nodejs` | Node.js client tests |
| `@dotnet` | .NET client tests |
| `@rust` | Rust protocol-level tests |
| `@copy-protocol` | COPY protocol tests |
| `@cancel` | Query cancellation tests |
| `@admin-commands` | Admin console commands |
| `@admin-leak` | Admin connection leak tests |
| `@buffer-cleanup` | Buffer cleanup tests |
| `@rollback` | Rollback functionality tests |
| `@hba` | HBA authentication tests |
| `@prometheus` | Prometheus metrics tests |

## Writing New Tests

Tests are organized as BDD feature files in `tests/bdd/features/`. Each feature file describes test scenarios using Gherkin syntax.

### Shell Tests (Recommended for Client Libraries)

Shell tests run external test commands (Go, Python, Node.js, etc.) and verify their output. This is the simplest way to test client library compatibility.

**Example** (`tests/bdd/features/my-feature.feature`):

```gherkin
@go @mytag
Feature: My feature description

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 40
      """

  Scenario: Test my Go client
    When I run shell command:
      """
      export DATABASE_URL="postgresql://example_user_1:test@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run TestMyTest ./mypackage
      """
    Then the command should succeed
    And the command output should contain "PASS"
```

**Test implementation** (in your preferred language):
- Go: `tests/go/mypackage/my_test.go`
- Python: `tests/python/test_my.py`
- Node.js: `tests/nodejs/my.test.js`
- .NET: `tests/dotnet/MyTest.cs`

### Rust Protocol-Level Tests

For testing PostgreSQL protocol behavior at the wire level, use Rust-based tests. These tests directly send and receive PostgreSQL protocol messages, allowing precise control and comparison.

**Example** (`tests/bdd/features/protocol-test.feature`):

```gherkin
@rust @my-protocol-test
Feature: Protocol behavior test
  Testing that pg_doorman handles protocol messages identically to PostgreSQL

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  @my-scenario
  Scenario: Query gives identical results from PostgreSQL and pg_doorman
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to both
    Then we should receive identical messages from both

  @session-test
  Scenario: Session management test
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "one"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    # ... more steps
```

**Available Rust test steps:**

Protocol comparison (sends to both PostgreSQL and pg_doorman):
- `we login to postgres and pg_doorman as "user" with password "pass" and database "db"`
- `we send SimpleQuery "SQL" to both`
- `we send CopyFromStdin "COPY ..." with data "..." to both`
- `we should receive identical messages from both`

Session management (for complex scenarios):
- `we create session "name" to pg_doorman as "user" with password "pass" and database "db"`
- `we send SimpleQuery "SQL" to session "name"`
- `we send SimpleQuery "SQL" to session "name" and store backend_pid`
- `we abort TCP connection for session "name"`
- `we sleep 100ms`

Cancel request testing:
- `we create session "name" ... and store backend key`
- `we send SimpleQuery "SQL" to session "name" without waiting for response`
- `we send cancel request for session "name"`
- `session "name" should receive cancel error containing "text"`

### Adding Dependencies

If you need additional packages in the test environment, modify `tests/nix/flake.nix`:
- Add Python packages to `pythonEnv`
- Add system packages to `runtimePackages`

After modifying `flake.nix`, rebuild the image with `make local-build`.

## Contribution Guidelines

### Code Style

- Follow the Rust style guidelines
- Use meaningful variable and function names
- Add comments for complex logic
- Write tests for new functionality

### Pull Request Process

1. **Create a new branch** for your feature or bugfix
2. **Make your changes** and commit them with clear, descriptive messages
3. **Write or update tests** as necessary
4. **Update documentation** to reflect any changes
5. **Submit a pull request** to the main repository
6. **Address any feedback** from code reviews

### Reporting Issues

If you find a bug or have a feature request, please create an issue on the [GitHub repository](https://github.com/ozontech/pg_doorman/issues) with:

- A clear, descriptive title
- A detailed description of the issue or feature
- Steps to reproduce (for bugs)
- Expected and actual behavior (for bugs)

## Getting Help

If you need help with your contribution, you can:

- Ask questions in the GitHub issues
- Reach out to the maintainers

Thank you for contributing to PgDoorman!
