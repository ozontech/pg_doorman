![pg_doorman](/static/logo_color_bg.png)

# PgDoorman

A high-performance multithreaded PostgreSQL connection pooler built in Rust. Does one thing and does it well — pools connections so your PostgreSQL handles thousands of clients without breaking a sweat.

## Why PgDoorman?

**Drop-in replacement. No app changes.** Most poolers in transaction mode break prepared statements, forcing you to rewrite application code. PgDoorman caches and remaps prepared statements transparently across server connections — just point your connection string at it and go. No `DISCARD ALL`, no `DEALLOCATE`, no driver hacks.

**Battle-tested with real drivers.** Two years of production use with Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), Node.js, and Rust. Protocol edge cases — pipelined batches, async Flush, Describe flow, cancel requests over TLS — are covered by comprehensive multi-language BDD tests.

**Natively multithreaded.** PgBouncer is single-threaded. Running multiple instances via `SO_REUSE_PORT` leads to unbalanced pools: clients connect evenly but disconnect unpredictably, leaving some instances overloaded while others sit idle. PgDoorman uses a single shared pool across all worker threads, ensuring correct connection distribution at any scale.

**Full extended query protocol support.** Odyssey does not fully support the PostgreSQL extended query protocol in transaction pooling mode, resulting in significantly degraded performance for modern drivers that rely on it. PgDoorman handles simple, extended, and prepared protocols equally well. See [full benchmarks](https://ozontech.github.io/pg_doorman/benchmarks.html).

## What sets PgDoorman apart

| Feature | PgDoorman | PgBouncer | Odyssey |
|---------|-----------|-----------|---------|
| Multithreaded | Yes | No (single-threaded) | Yes |
| Transparent prepared statements in transaction mode | Yes (LRU cache, xxhash3) | No | No |
| Extended query protocol | Full support | Full support | Partial (broken in transaction mode) |
| Zero-downtime binary upgrade | Yes (`SIGINT` + `SO_REUSE_PORT`) | Yes (similar) | No |
| Deferred `BEGIN` (skip server acquire) | Yes | No | No |
| Dead connection detection (`server_idle_check_timeout`) | Yes (sends `;` probe) | No | No |
| `server_lifetime` jitter (±20%) | Yes (prevents thundering herd) | No | No |
| Config validation (`-t` flag) | Yes (nginx-style) | No | No |
| Auto-config from PostgreSQL | Yes (`generate` command) | No | No |
| Human-readable durations/sizes | Yes (`"3s"`, `"256MB"`) | No | No |
| YAML config support | Yes (recommended) | No (INI only) | No (custom format) |
| Prometheus metrics | Built-in | Requires exporter | Built-in |
| `pg_hba.conf` access control | Yes (native format) | Yes (simplified) | No |
| PAM / JWT (Talos) auth | Yes | No | PAM only |
| COPY protocol streaming | Yes (chunked, no OOM) | Yes | Yes |
| Config format | YAML + TOML | INI | Custom |

## Quick Start

```bash
# Generate config from your existing PostgreSQL (auto-detects databases and users)
pg_doorman generate --host your-db-host --output pg_doorman.yaml

# Start
pg_doorman pg_doorman.yaml

# Connect (same as you would to PostgreSQL)
psql -h localhost -p 6432 -U youruser yourdb
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

## Features

- **Transaction pooling** with transparent prepared statement caching (LRU, xxhash3)
- **Deferred `BEGIN`** — standalone `BEGIN` doesn't acquire a server connection until the next query
- **Dead connection detection** — idle server connections are probed before reuse (`server_idle_check_timeout`)
- **Connection lifetime jitter** (±20%) — prevents thundering herd of simultaneous reconnections
- **YAML and TOML** config with annotated generation (`generate --reference`)
- **Human-readable values** — `connect_timeout: "3s"`, `max_memory_usage: "256MB"`
- **Zero-downtime binary upgrades** via `SIGINT` + `SO_REUSE_PORT` (foreground and daemon modes)
- **Config test mode** (`-t`) — nginx-style validation without starting the server
- **pg_hba.conf** access control, TLS, PAM and JWT authentication
- **Prometheus metrics** built-in (port 9127)
- **Admin console** — `psql -p 6432 -U admin pgdoorman` then `SHOW POOLS`, `RELOAD`, etc.
- **patroni_proxy** — included TCP proxy for Patroni clusters with zero-downtime failover

## Documentation

Full documentation, configuration reference, and tutorials: **[ozontech.github.io/pg_doorman](https://ozontech.github.io/pg_doorman/)**

## Contributing

```bash
make pull       # pull test image
make test-bdd   # run all integration tests (Docker-based, fully reproducible)
```

See the [Contributing Guide](https://ozontech.github.io/pg_doorman/tutorials/contributing.html) for details.
