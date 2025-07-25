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

5. **Run tests**:
   ```bash
   cargo test
   ```

6. **Run integration tests with Docker**:
   ```bash
   make docker-compose-test-all
   ```

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