# PgDoorman

A multi-threaded PostgreSQL connection pooler written in Rust. Drop-in replacement for [PgBouncer](https://www.pgbouncer.org/) and [Odyssey](https://github.com/yandex/odyssey), and an alternative to [PgCat](https://github.com/postgresml/pgcat). Three years in production at Ozon under Go (pgx), .NET (Npgsql), Python (asyncpg, SQLAlchemy), and Node.js workloads.

[Get PgDoorman {{VERSION}}](tutorials/installation.md) · [Comparison](comparison.md) · [Benchmarks](benchmarks.md)

## Headline features

```admonish success title="Pool Coordinator"
PgDoorman caps total backend connections per database. When `max_db_connections` is reached, the coordinator evicts an idle connection from the user with the most spare capacity, ranking candidates by p95 transaction time so the slowest pools yield first. A reserve pool absorbs short bursts; per-user `min_guaranteed_pool_size` keeps critical workloads off the eviction list.

PgBouncer's `max_db_connections` has no eviction or fairness — when the cap is reached, clients queue until existing connections close on their own idle timeout. Odyssey has no equivalent setting.

[Read more →](concepts/pool-coordinator.md)
```

```admonish success title="Patroni-assisted Fallback"
When PgDoorman runs next to PostgreSQL on the same machine and a Patroni switchover kills the local backend, PgDoorman polls the Patroni REST API (`GET /cluster`), picks a live cluster member (priority `sync_standby` → `replica`), and routes new connections there. The local backend enters cooldown; fallback connections inherit a short lifetime so the pool returns to local as soon as it recovers.

Set `patroni_api_urls` and `fallback_cooldown` in `[general]` and it applies to every pool. No HAProxy or `consul-template` in front of the pooler.

[Read more →](tutorials/patroni-assisted-fallback.md)
```

```admonish success title="Graceful Binary Upgrade"
Update PgDoorman during business hours without a maintenance window. Apps don't see reconnect errors, PostgreSQL isn't hit by a wave of `auth`/SCRAM handshakes from simultaneous reconnects, in-flight transactions don't fail.

On `SIGUSR2` the old process hands each idle client's TCP socket to the new one through `SCM_RIGHTS` — same socket, no reconnect — together with cancel keys and the prepared-statement cache. Clients inside a transaction finish on the old process and migrate as soon as they go idle. With the `tls-migration` build (Linux, opt-in) the OpenSSL cipher state moves too, so TLS sessions survive without a re-handshake.

PgBouncer's online restart (`-R`, deprecated since 1.20; or `so_reuseport` rolling restart) and Odyssey's online restart (`SIGUSR2` + `bindwith_reuseport`) work the same way as each other: the new process picks up new connections, the old one drains until its existing clients disconnect on their own. Sessions, prepared statements, and TLS state never move between processes.

[Read more →](tutorials/binary-upgrade.md)
```

## Why PgDoorman

- **Prepared statements in transaction mode.** PgDoorman remaps client statement names to `DOORMAN_N` and tracks the cache per pool, per client, and per backend. Drivers see their own names; backends see the remapped ones. No app-level `DEALLOCATE`, no `DISCARD ALL`.
- **Multi-threaded, single shared pool.** All worker threads share one pool. PgBouncer is single-threaded; the recommended scale-out — several instances behind `so_reuseport` — gives each instance its own pool, and idle counts can drift between processes for the same database.
- **Thundering herd suppression.** When 200 clients race for 4 idle connections, PgDoorman caps concurrent backend creates (`scaling_max_parallel_creates`) and routes returning servers straight to the longest-waiting client through an in-process oneshot channel — no requeue through the idle pool.
- **Bounded tail latency.** Waiters are served strict FIFO so the worst-case wait can't be overtaken by latecomers. Pre-replacement of expiring backends — at 95% of `server_lifetime`, up to 3 in parallel — keeps the pool warm, so there is no checkout spike when a generation of connections rotates out.
- **Dead backend detection inside transactions.** If the backend dies mid-transaction (failover, OOM, network partition), PgDoorman returns SQLSTATE `08006` immediately by racing the client read against backend readability with a 100 ms tick. Without this, the client would block until TCP keepalive fires — on Linux defaults that is about two hours plus 9×75 s probes.
- **Built for operations.** YAML or TOML config with human-readable durations (`30s`, `5m`). `pg_doorman generate --host …` introspects an existing PostgreSQL and emits a starter config. `pg_doorman -t` validates the config without starting the server. A Prometheus `/metrics` endpoint is built-in.

## Comparison

| Feature                                                  |       PgDoorman       |              PgBouncer              |          Odyssey           |
| -------------------------------------------------------- | :-------------------: | :---------------------------------: | :------------------------: |
| Multi-threaded with shared pool                          |          Yes          |        No (single-threaded)         |  Workers, separate pools   |
| Prepared statements in transaction mode                  |          Yes          |           Yes (since 1.21)          | Yes (`pool_reserve_prepared_statement`) |
| Pool Coordinator (per-database cap, priority eviction)   |          Yes          |                 No                  |             No             |
| Patroni-assisted fallback (built-in)                     |          Yes          |                 No                  |             No             |
| Pre-replacement on `server_lifetime` expiry              |          Yes          |                 No                  |             No             |
| Stale backend detection inside a transaction             | Yes (immediate `08006`) |     No (waits for TCP keepalive)    | No (waits for TCP keepalive) |
| Binary upgrade with session migration                    | Yes (`SCM_RIGHTS`, TLS state opt-in) | No (sessions stay on old process) | No (sessions stay on old process) |
| Backend TLS to PostgreSQL                                |   Yes (5 modes, hot reload via `SIGHUP`) | Yes (`server_tls_*`, hot reload via `RELOAD`) |             No             |
| Auth: SCRAM passthrough (no plaintext password)          | Yes (`ClientKey` extracted from proof) | Yes (encrypted SCRAM secret via `auth_query`/`userlist.txt`, since 1.14) |            Yes             |
| Auth: JWT (RSA-SHA256)                                   |          Yes          |                 No                  |             No             |
| Auth: PAM / `pg_hba.conf` / `auth_query`                 |          Yes          |                 Yes                 |            Yes             |
| Auth: LDAP                                               |          No           |           Yes (since 1.25)          |            Yes             |
| Config format                                            |     YAML / TOML       |                 INI                 |        Own format          |
| JSON structured logging                                  |          Yes          |                 No                  |   Yes (`log_format "json"`)   |
| Latency percentiles (p50/p90/p95/p99)                    | Yes (built-in `/metrics`) |        No (averages only)         |  Yes (via separate Go exporter) |
| Config test mode (`-t`)                                  |          Yes          |                 No                  |             No             |
| Auto-config from PostgreSQL (`generate --host`)          |          Yes          |                 No                  |             No             |
| Prometheus endpoint                                      | Built-in `/metrics`   |          External exporter          |  External exporter (Go sidecar) |

[Full feature matrix →](comparison.md)

## Benchmarks

AWS Fargate (16 vCPU), pool size 40, `pgbench` 30 s per test:

| Scenario                                | vs PgBouncer | vs Odyssey |
| --------------------------------------- | :----------: | :--------: |
| Extended protocol, 500 clients + SSL    |     ×3.5     |    +61%    |
| Prepared statements, 500 clients + SSL  |     ×4.0     |    +5%     |
| Simple protocol, 10 000 clients         |     ×2.8     |    +20%    |
| Extended + SSL + reconnect, 500 clients |     +96%     |    ~0%     |

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

`server_username` and `server_password` are omitted on purpose: PgDoorman re-uses the client's MD5 hash or SCRAM `ClientKey` to authenticate against PostgreSQL. No plaintext passwords in the config.

[Installation guide →](tutorials/installation.md) · [Configuration reference →](reference/general.md)

## Where to next

- New to PgDoorman? Start with [Overview](tutorials/overview.md), then [Installation](tutorials/installation.md) and [Basic usage](tutorials/basic-usage.md).
- Migrating from PgBouncer or Odyssey? Read [Comparison](comparison.md) and [Authentication](authentication/overview.md).
- Running Patroni? See [Patroni-assisted fallback](tutorials/patroni-assisted-fallback.md) and [`patroni_proxy`](tutorials/patroni-proxy.md).
- Production sizing? Read [Pool pressure](tutorials/pool-pressure.md) and [Pool Coordinator](concepts/pool-coordinator.md).
- Operating PgDoorman? See [Binary upgrade](tutorials/binary-upgrade.md), [Signals](operations/signals.md), [Troubleshooting](tutorials/troubleshooting.md).
