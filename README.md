![pg_doorman](/static/logo_color_bg.png)

# PgDoorman

[![BDD Tests](https://github.com/ozontech/pg_doorman/actions/workflows/bdd-tests.yml/badge.svg)](https://github.com/ozontech/pg_doorman/actions/workflows/bdd-tests.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-ozontech.github.io%2Fpg__doorman-blue)](https://ozontech.github.io/pg_doorman/)
[![Telegram](https://img.shields.io/badge/telegram-%40pg__doorman-blue?logo=telegram)](https://t.me/pg_doorman)

A multithreaded PostgreSQL connection pooler in Rust (MSRV 1.87). Alternative to PgBouncer, Odyssey, and PgCat. In production at Ozon for over three years across Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), and Node.js workloads.

> ## 📖 Full documentation: **[ozontech.github.io/pg_doorman](https://ozontech.github.io/pg_doorman/)**
>
> Configuration reference, tutorials, comparison with PgBouncer and Odyssey, benchmarks, deployment guides — all on the documentation site.
>
> **Available in [English](https://ozontech.github.io/pg_doorman/) and [Русский](https://ozontech.github.io/pg_doorman/ru/).**

## Quick start

```bash
docker run -p 6432:6432 \
  -v $(pwd)/pg_doorman.yaml:/etc/pg_doorman/pg_doorman.yaml \
  ghcr.io/ozontech/pg_doorman
```

Generate a config from your PostgreSQL:

```bash
pg_doorman generate --host db.example.com --user postgres --output pg_doorman.yaml
```

Connect and verify:

```bash
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c "SHOW VERSION;"
```

For installation from source, distribution packages (Ubuntu PPA, Fedora COPR), and the `tls-migration` build, see the [Installation guide](https://ozontech.github.io/pg_doorman/tutorials/installation.html).

## Highlights

- **Multithreaded** — one process, one shared pool across all worker threads.
- **Anonymous prepared statements get plan caching** — PgBouncer 1.21+ and Odyssey forward the empty-name `Parse` unchanged and re-plan on every `Bind`; pg_doorman remaps it to `DOORMAN_<N>` so the plan lands in the backend's named registry and gets reused across clients sharing the pool. [Learn more](https://ozontech.github.io/pg_doorman/tutorials/prepared-statements.html).
- **Pool Coordinator** — database-level connection cap with priority eviction and per-user minimums. [Learn more](https://ozontech.github.io/pg_doorman/concepts/pool-coordinator.html).
- **Patroni-assisted fallback** — automatic backend rerouting via the Patroni REST API when the local node fails. [Learn more](https://ozontech.github.io/pg_doorman/tutorials/patroni-assisted-fallback.html).
- **Graceful binary upgrade** — replace the binary without dropping clients; TLS connections migrate cleanly with the `tls-migration` cargo feature. [Learn more](https://ozontech.github.io/pg_doorman/tutorials/binary-upgrade.html).
- **Built-in observability** — Prometheus `/metrics` with HDR-histogram percentiles, structured JSON logs, admin `SHOW` commands.

[Full feature comparison →](https://ozontech.github.io/pg_doorman/comparison.html)

## Benchmarks

Continuously updated `pgbench` results from AWS Fargate and Ubicloud (multiple scenarios) live on the [benchmarks page](https://ozontech.github.io/pg_doorman/benchmarks.html). PgDoorman runs 3-4× ahead of PgBouncer on extended-protocol and prepared-statement workloads, and noticeably ahead of Odyssey under the same scenarios.

## Community

- **Telegram:** [@pg_doorman](https://t.me/pg_doorman)
- **Issues:** [github.com/ozontech/pg_doorman/issues](https://github.com/ozontech/pg_doorman/issues)
- **Contributing:** [contributing guide](https://ozontech.github.io/pg_doorman/tutorials/contributing.html)

## License

MIT — see [LICENSE](LICENSE).
