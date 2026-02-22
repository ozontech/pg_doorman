![pg_doorman](/static/logo_color_bg.png)

# PgDoorman

A high-performance PostgreSQL connection pooler built in Rust. Does one thing and does it well — pools connections so your PostgreSQL handles thousands of clients without breaking a sweat.

## Why PgDoorman?

**Drop-in replacement. No app changes.** Most poolers in transaction mode break prepared statements, forcing you to rewrite application code. PgDoorman handles prepared statements transparently — just point your connection string at it and go.

**Battle-tested with real drivers.** Two years of production use with Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), Node.js, and Rust. Protocol edge cases that crash other poolers are covered by extensive multi-language integration tests.

**Fast.** Up to 3-4x faster than PgBouncer under load. At 10,000 concurrent clients with SSL: 3.5x throughput on extended protocol, 4x on prepared statements. See [full benchmarks](https://ozontech.github.io/pg_doorman/benchmarks.html).

## Quick Start

```bash
# Generate config from your existing PostgreSQL (auto-detects databases and users)
pg_doorman generate --host your-db-host --output pg_doorman.yaml

# Start
pg_doorman pg_doorman.yaml

# Connect (same as you would to PostgreSQL)
psql -h localhost -p 6432 -U youruser yourdb
```

Or install from packages:

```bash
# Ubuntu/Debian
sudo add-apt-repository ppa:vadv/pg-doorman && sudo apt-get install pg-doorman

# Fedora/RHEL/Rocky
sudo dnf copr enable vadvya/pg-doorman && sudo dnf install pg-doorman

# Docker
docker pull ghcr.io/ozontech/pg_doorman
```

## Features

- **Transaction pooling** with transparent prepared statement support
- **YAML and TOML** config formats with annotated config generation (`generate --reference`)
- **Zero-downtime binary upgrades** via `SIGINT` + `SO_REUSE_PORT`
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
