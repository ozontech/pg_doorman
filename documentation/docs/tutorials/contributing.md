---
title: Contributing to PgDoorman
---

# Contributing to PgDoorman

Thank you for your interest in contributing to PgDoorman! This guide will help you set up your development environment and understand the contribution process.

## Getting Started

### Prerequisites

Before you begin, make sure you have the following installed:

- [Rust](https://www.rust-lang.org/tools/install) (latest stable version)
- [Git](https://git-scm.com/downloads)
- [Docker](https://docs.docker.com/get-docker/) (optional, for running tests)
- [Make](https://www.gnu.org/software/make/) (optional, for running test scripts)

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

PgDoorman uses BDD (Behavior-Driven Development) tests with a Docker-based test environment. **Nix installation is NOT required** — everything runs inside Docker containers.

### Test Environment

The test Docker image (built with Nix) includes:
- PostgreSQL 16
- Go 1.24
- Python 3 with asyncpg, psycopg2, aiopg, pytest
- Node.js 22
- .NET SDK 8
- Rust 1.87.0

### Running Tests

Navigate to the `tests` directory and use Make:

```bash
cd tests

# Pull the test image from registry
make pull

# Or build locally (takes 10-15 minutes on first run)
make local-build

# Run all BDD tests
make test-bdd

# Run tests for specific language
make test-bdd-go          # Go client tests
make test-bdd-python      # Python client tests
make test-bdd-nodejs      # Node.js client tests
make test-bdd-dotnet      # .NET client tests

# Run tests for specific feature
make test-bdd-hba         # HBA authentication tests
make test-bdd-prometheus  # Prometheus metrics tests
make test-bdd-rollback    # Rollback functionality tests

# Open interactive shell in test container
make shell

# Build pg_doorman inside container
make tests-build
```

You can also use `tests/nix/run-tests.sh` directly:

```bash
./tests/nix/run-tests.sh bdd              # Run all BDD tests
./tests/nix/run-tests.sh bdd @go          # Run tests tagged with @go
./tests/nix/run-tests.sh bdd @python      # Run tests tagged with @python
./tests/nix/run-tests.sh shell            # Interactive shell
./tests/nix/run-tests.sh help             # Show all available commands
```

### Writing New Tests

Tests are organized as BDD feature files in `tests/bdd/features/`. Each feature file describes test scenarios using Gherkin syntax.

#### Structure

1. **Feature file** (`tests/bdd/features/my-feature.feature`):
   ```gherkin
   @mytag
   Feature: My feature description
   
     Background:
       Given PostgreSQL started with pg_hba.conf:
         """
         host all all 127.0.0.1/32 trust
         """
       And pg_doorman started with config:
         """
         [general]
         host = "127.0.0.1"
         port = ${DOORMAN_PORT}
         ...
         """
   
     Scenario: Test something
       When I run shell command:
         """
         cd tests/go && go test -v -run TestMyTest ./mypackage
         """
       Then the command should succeed
       And the command output should contain "PASS"
   ```

2. **Test implementation** (in your preferred language):
   - Go: `tests/go/mypackage/my_test.go`
   - Python: `tests/python/test_my.py`
   - Node.js: `tests/nodejs/my.test.js`
   - .NET: `tests/dotnet/MyTest.cs`

#### Adding Dependencies

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