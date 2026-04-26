# PgDoorman

A multithreaded PostgreSQL connection pooler written in Rust. Drop-in alternative to [PgBouncer](https://www.pgbouncer.org/), [Odyssey](https://github.com/yandex/odyssey), and [PgCat](https://github.com/postgresml/pgcat). In production at Ozon for over three years across Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), and Node.js workloads.

[Get PgDoorman {{VERSION}}](tutorials/installation.md) · [Compare](comparison.md) · [Benchmarks](benchmarks.md)

## What makes PgDoorman different

Three things you will not find in PgBouncer or Odyssey.

```admonish success title="Pool Coordinator"
Database-level connection cap with priority eviction. `max_db_connections` limits total backend connections per database; when exhausted, idle connections are evicted from users with the largest surplus, ranked by p95 transaction time so slow pools donate first. A reserve pool absorbs short bursts. Per-user `min_guaranteed_pool_size` protects critical workloads.

PgBouncer has `max_db_connections` without eviction or fairness. Odyssey has no equivalent.

[Read more →](concepts/pool-coordinator.md)
```

```admonish success title="Patroni-assisted Fallback"
When PgDoorman runs next to PostgreSQL on the same machine and a Patroni switchover kills the local backend, PgDoorman queries the Patroni REST API `/cluster` endpoint, picks a live cluster member (`sync_standby` preferred), and routes new connections there within 1–2 TCP round trips. The local backend stays in cooldown; fallback connections use a short lifetime so the pool returns to local once it recovers.

One line in `[general]` enables it for every pool. No external HAProxy, no consul-template.

[Read more →](tutorials/patroni-assisted-fallback.md)
```

```admonish success title="Graceful Binary Upgrade"
Replace the binary without dropping a single client. The new process accepts new connections immediately while existing clients finish their transactions on the old process. TLS, connection state, and cancel keys all transfer cleanly.

PgBouncer requires `SO_REUSEPORT` with separate processes (which causes pool imbalance). Odyssey lacks an equivalent flow.

[Read more →](tutorials/binary-upgrade.md)
```

## Why PgDoorman

- **Drop-in replacement.** Caches and remaps prepared statements transparently in transaction mode — no `DISCARD ALL`, no `DEALLOCATE`, no driver hacks.
- **Multithreaded.** Single shared pool across all worker threads. PgBouncer is single-threaded; running multiple instances via `SO_REUSEPORT` causes unbalanced pools.
- **Thundering herd suppression.** When 200 clients race for 4 idle connections, PgDoorman caps concurrent backend creates and routes waiters to recycled connections via direct handoff — most land within microseconds.
- **Bounded tail latency.** Strict FIFO via direct-handoff channels keeps p99 within 10% of p50 regardless of client count. Pre-replacement on `server_lifetime` expiry — no spike during connection rotation.
- **Dead backend detection.** When a client holds a transaction open and the backend dies (failover, OOM kill), PgDoorman returns an error immediately. Other poolers wait for TCP keepalive, leaving clients hanging for minutes.
- **Built for operations.** YAML or TOML config with human-readable durations (`"30s"`, `"5m"`). `pg_doorman generate --host your-db` introspects PostgreSQL to produce a config. `pg_doorman -t` validates before deploy. Prometheus endpoint is built-in.

## Comparison

|                                                          |    PgDoorman    | PgBouncer | Odyssey |
| -------------------------------------------------------- | :-------------: | :-------: | :-----: |
| Multithreaded                                            |       Yes       |    No     |   Yes   |
| Prepared statements in transaction mode                  |       Yes       | Since 1.21 | Since 1.3 |
| Full extended query protocol                             |       Yes       |    Yes    | Partial |
| Pool Coordinator with priority eviction                  |       Yes       |    No     |   No    |
| Patroni-assisted fallback (built-in)                     |       Yes       |    No     |   No    |
| Pre-replacement on `server_lifetime` expiry              |       Yes       |    No     |   No    |
| Stale backend detection (idle-in-transaction)            |       Yes       |    No     |   No    |
| Graceful binary upgrade                                  |       Yes       |  Limited  |   No    |
| Server-side TLS (mTLS, hot reload)                       |       Yes       |    No     |   No    |
| Auth: SCRAM passthrough (no plaintext password in config) |      Yes       |    No     |   Yes   |
| Auth: JWT                                                |       Yes       |    No     |   No    |
| Auth: PAM / `pg_hba.conf` / auth_query                   |       Yes       |   Yes     |   Yes   |
| Auth: LDAP                                               |       No        | Since 1.25 |   Yes  |
| YAML / TOML config                                       |       Yes       | No (INI)  | No (own format) |
| JSON structured logging                                  |       Yes       |    No     |   Yes   |
| Latency percentiles (p50/90/95/99)                       |       Yes       |    No     |   Yes   |
| Config test mode (`-t`)                                  |       Yes       |    No     |   No    |
| Auto-config from PostgreSQL                              |       Yes       |    No     |   No    |
| Built-in Prometheus endpoint                             |       Yes       | External  |   Yes   |

[Full feature matrix →](comparison.md)

## Benchmarks

AWS Fargate (16 vCPU), pool size 40, `pgbench` 30s per test:

| Scenario                                | vs PgBouncer | vs Odyssey |
| --------------------------------------- | :----------: | :--------: |
| Extended protocol, 500 clients + SSL    |     ×3.5     |    +61%    |
| Prepared statements, 500 clients + SSL  |     ×4.0     |    +5%     |
| Simple protocol, 10 000 clients         |     ×2.8     |    +20%    |
| Extended + SSL + Reconnect, 500 clients |    +96%     |    ~0%     |

[Full results →](benchmarks.md)

## Quick start

Run via Docker:

```bash
docker run -p 6432:6432 \
  -v $(pwd)/pg_doorman.yaml:/etc/pg_doorman/pg_doorman.yaml \
  ghcr.io/ozontech/pg_doorman
```

Minimal config (`pg_doorman.yaml`):

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
        password: "md5..."   # hash from pg_shadow / pg_authid
        pool_size: 40
```

`server_username` and `server_password` are omitted on purpose — PgDoorman reuses the client's MD5 hash or SCRAM ClientKey to authenticate to PostgreSQL. No plaintext passwords in config.

[Installation guide →](tutorials/installation.md) · [Configuration reference →](reference/general.md)

## Where to next

- New to PgDoorman? Start with [Overview](tutorials/overview.md), then [Installation](tutorials/installation.md) and [Basic usage](tutorials/basic-usage.md).
- Migrating from PgBouncer or Odyssey? Read [Comparison](comparison.md) and [Authentication](authentication/overview.md).
- Running Patroni? See [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md) and [`patroni_proxy`](tutorials/patroni-proxy.md).
- Production sizing? Read [Pool pressure](tutorials/pool-pressure.md) and [Pool Coordinator](concepts/pool-coordinator.md).
- Operating PgDoorman? See [Binary upgrade](tutorials/binary-upgrade.md), [Signals](operations/signals.md), [Troubleshooting](tutorials/troubleshooting.md).
