![pg_doorman](/static/logo_color_bg.png)

# PgDoorman

A high-performance multithreaded PostgreSQL connection pooler built in Rust. Does one thing and does it well — pools connections so your PostgreSQL handles thousands of clients without breaking a sweat.

## Why PgDoorman?

**Drop-in replacement. No app changes.** Most poolers in transaction mode break prepared statements, forcing you to rewrite application code. PgDoorman caches and remaps prepared statements transparently across server connections — just point your connection string at it and go. No `DISCARD ALL`, no `DEALLOCATE`, no driver hacks.

**Battle-tested with real drivers.** Free years of production use with Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), Node.js. Protocol edge cases — pipelined batches, async Flush, Describe flow, cancel requests over TLS — are covered by comprehensive multi-language BDD tests.

**Natively multithreaded.** PgBouncer is single-threaded. Running multiple instances via `SO_REUSE_PORT` leads to unbalanced pools: clients connect evenly but disconnect unpredictably, leaving some instances overloaded while others sit idle. PgDoorman uses a single shared pool across all worker threads, ensuring correct connection distribution at any scale.

**Full extended query protocol support.** Odyssey does not fully support the PostgreSQL extended query protocol in transaction pooling mode, resulting in significantly degraded performance for modern drivers that rely on it. PgDoorman handles simple, extended, and prepared protocols equally well. See [full benchmarks](https://ozontech.github.io/pg_doorman/benchmarks.html).

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

## Features

- **Transaction pooling** with transparent prepared statement caching (LRU, xxhash3)
- **Deferred `BEGIN`** — standalone `BEGIN` doesn't acquire a server connection until the next query
- **Zero-downtime binary upgrades** via `SIGINT` + `SO_REUSE_PORT` (foreground and daemon modes)
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
