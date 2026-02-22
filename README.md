![pg_doorman](/static/logo_color_bg.png)

# PgDoorman

[![BDD Tests](https://github.com/ozontech/pg_doorman/actions/workflows/bdd-tests.yml/badge.svg)](https://github.com/ozontech/pg_doorman/actions/workflows/bdd-tests.yml)
[![Library Tests](https://github.com/ozontech/pg_doorman/actions/workflows/lib-tests.yml/badge.svg)](https://github.com/ozontech/pg_doorman/actions/workflows/lib-tests.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A high-performance multithreaded PostgreSQL connection pooler built in Rust. Does one thing and does it well — pools connections so your PostgreSQL handles thousands of clients without breaking a sweat.

## Why PgDoorman?

**Drop-in replacement. No app changes.** PgDoorman caches and remaps prepared statements transparently across server connections in transaction mode — just point your connection string at it and go. No `DISCARD ALL`, no `DEALLOCATE`, no driver hacks. PgBouncer added similar support in 1.21, but remains single-threaded; Odyssey added it in 1.3, but has known reliability issues in edge cases.

**Battle-tested with real drivers.** Two years of production use with Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), Node.js. Protocol edge cases — pipelined batches, async Flush, Describe flow, cancel requests over TLS — are covered by comprehensive multi-language BDD tests.

**Natively multithreaded.** PgBouncer is single-threaded. Running multiple instances via `SO_REUSE_PORT` leads to unbalanced pools: clients connect evenly but disconnect unpredictably, leaving some instances overloaded while others sit idle. PgDoorman uses a single shared pool across all worker threads, ensuring correct connection distribution at any scale.

**Full extended query protocol support.** Benchmarks show Odyssey is up to 61% slower with the extended query protocol in transaction mode. PgDoorman handles simple, extended, and prepared protocols equally well — including pipelined batches and async Flush flow that cause issues in other poolers.

## Benchmarks

Automated benchmarks on AWS Fargate (16 vCPU, pool size 40, pgbench 30s per test):

| Scenario | vs PgBouncer | vs Odyssey |
|----------|-------------|------------|
| Extended protocol, 500 clients + SSL | x3.5 | +61% |
| Prepared statements, 500 clients + SSL | x4.0 | +5% |
| Simple protocol, 10,000 clients | x2.8 | +20% |
| Extended + SSL + Reconnect, 500 clients | +96% | ~0% |

PgBouncer is single-threaded — these ratios reflect a single PgBouncer instance vs a single PgDoorman instance. [Full benchmark results](https://ozontech.github.io/pg_doorman/benchmarks.html).

## Comparison

| | PgDoorman | PgBouncer | Odyssey |
|---|:-:|:-:|:-:|
| Multithreaded | Yes | No | Yes |
| Prepared statements in transaction mode | Yes | Since 1.21 | Since 1.3 |
| Full extended query protocol | Yes | Yes | Partial |
| Zero-downtime binary upgrade | Yes | Yes | Yes |
| Deferred `BEGIN` (lazy server acquire) | Yes | No | No |
| Auto-config from PostgreSQL | Yes | No | No |
| YAML / TOML config | Yes | No (INI) | No (own format) |
| Human-readable durations & sizes | Yes | No | No |
| Native `pg_hba.conf` format | Yes | Yes | Since 1.4 |
| PAM auth | Yes | Yes | Yes |
| LDAP auth | No | Since 1.25 | Yes |
| Prometheus metrics | Built-in | External | Built-in |

## Quick Start

### Minimal config

```yaml
general:
  host: "0.0.0.0"
  port: 6432
  admin_username: "admin"
  admin_password: "change_me"

pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "app"
        password: "md5..."           # hash for client auth (from pg_shadow)
        pool_size: 40
        server_username: "app"       # real PostgreSQL username
        server_password: "secret"    # real PostgreSQL password (plaintext)
```

> **Important:** `server_username` and `server_password` are required if your client `password` is an MD5/SCRAM hash (which is typical). Without them, PgDoorman tries to authenticate to PostgreSQL using the hash itself, and PostgreSQL rejects it. This is the #1 setup issue for new users.

Or generate a config automatically:

```bash
pg_doorman generate --host your-db-host --output pg_doorman.yaml
```

### Run and connect

```bash
# Start
pg_doorman pg_doorman.yaml

# Connect — same as you would to PostgreSQL directly
psql -h localhost -p 6432 -U app mydb
```

Your application connection string changes only the host and port:

```
postgresql://app:secret@localhost:6432/mydb
```

## Installation

**Pre-built binaries:** Download from [GitHub Releases](https://github.com/ozontech/pg_doorman/releases).

```bash
# Ubuntu/Debian
sudo add-apt-repository ppa:vadv/pg-doorman && sudo apt-get install pg-doorman

# Fedora/RHEL/Rocky
sudo dnf copr enable vadvya/pg-doorman && sudo dnf install pg-doorman

# Docker
docker pull ghcr.io/ozontech/pg_doorman
```

### Building from source

```bash
# Recommended: build with jemalloc tuning for optimal memory management
JEMALLOC_SYS_WITH_MALLOC_CONF="dirty_decay_ms:30000,muzzy_decay_ms:30000,background_thread:true,metadata_thp:auto" \
  cargo build --release

# Binary will be at target/release/pg_doorman
```

## patroni_proxy

This repository also includes `patroni_proxy` — a TCP proxy for Patroni-managed PostgreSQL clusters. Zero-downtime failover: existing connections are preserved during cluster topology changes.

<p align="center">
  <img src="static/patroni_proxy_architecture.png" alt="patroni_proxy architecture" width="700" />
</p>

- **pg_doorman** deploys on the same host as PostgreSQL — connection pooling and prepared statement caching benefit from low latency to the database
- **patroni_proxy** deploys as a sidecar in the application pod — TCP routing and role-based failover (leader/sync/async) with least-connections balancing

See [patroni_proxy documentation](https://ozontech.github.io/pg_doorman/tutorials/patroni-proxy.html) for details.

## Documentation

Full documentation, configuration reference, and tutorials: **[ozontech.github.io/pg_doorman](https://ozontech.github.io/pg_doorman/)**

## Contributing

```bash
make pull       # pull test image
make test-bdd   # run all integration tests (Docker-based, fully reproducible)
```

See the [Contributing Guide](https://ozontech.github.io/pg_doorman/tutorials/contributing.html) for details.
