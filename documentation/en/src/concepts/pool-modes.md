# Pool Modes

PgDoorman supports two pool modes: `transaction` and `session`. Set per pool, with optional per-user override.

There is no `statement` mode.

## Transaction mode (recommended)

```yaml
pools:
  mydb:
    pool_mode: "transaction"
```

Supports:

- Named and anonymous prepared statements.
- Pipelined batches and asynchronous `Flush`.
- Query cancellation over TLS.

## Session mode

```yaml
pools:
  legacy_app:
    pool_mode: "session"
```

Use for clients that need:

- Session parameters (`SET search_path`, `SET TIME ZONE`).
- `LISTEN` subscriptions.
- `WITH HOLD` cursors and advisory locks held across transactions.

## Per-user override

A pool's mode can be overridden per user:

```yaml
pools:
  mydb:
    pool_mode: "transaction"
    users:
      - username: "app"
        password: "md5..."
        pool_size: 40
      - username: "admin_tools"
        password: "md5..."
        pool_size: 4
        pool_mode: "session"
```

Useful when one user (operations tooling, migrations) needs session semantics but the main application stays in transaction mode.

## Cleanup on checkin

The default is `cleanup_server_connections: adaptive` without `cleanup_server_query`: pg_doorman cleans a session only when it sees the session state change, so the pooler's cached prepared statements survive a checkin.

The built-in cleanup tracks five kinds of session state and answers with cleanup SQL:

- `SET` → `RESET ALL`.
- `DECLARE` cursor → `CLOSE ALL`.
- SQL `PREPARE` → `DEALLOCATE ALL`. SQL `PREPARE` and an extended-protocol `Parse` share one statement namespace.
- `LISTEN` → `UNLISTEN *`.
- `CREATE TEMP TABLE` → `DISCARD TEMP`.

Every cleanup batch also releases advisory locks with `pg_advisory_unlock_all()`.

Not tracked: `SELECT ... INTO TEMP` and `CREATE TEMP TABLE AS SELECT` — they complete with the inner query's tag. Temporary objects created inside functions carry no tag. Use `always` with suitable SQL to release these.

In transaction pooling the backend returns to the pool after every query. A temporary table lives for one transaction; keep a transaction open across the statements that use it.

`cleanup_server_query` replaces the built-in cleanup and requires `always`: your SQL runs on every checkin of a backend that served a client. It is incompatible with `adaptive`, and validation rejects that pairing. The mode costs the server one extra query per checkin. With `DISCARD ALL`, prepared statements are also removed and re-parsed after every checkin. `off` disables session cleanup. Open transactions are rolled back in every mode.

See the [pool reference](../reference/pool.md#cleanup_server_connections) for the full list of tracked statements, limitations, and PostgreSQL and Greengage examples.

## Reference

- `pool_mode` parameter: [Pool Settings](../reference/pool.md#pool_mode).
- `cleanup_server_connections`: [Pool Settings](../reference/pool.md#cleanup_server_connections).
- Pool sizing: [Pool Coordinator](pool-coordinator.md), [Pool Pressure](../tutorials/pool-pressure.md).
