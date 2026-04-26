# Pool Modes

PgDoorman supports two pool modes: `transaction` and `session`. Set per pool, with optional per-user override.

There is no `statement` mode. Statement-level pooling breaks more drivers than it helps and PgDoorman optimizes for transaction mode aggressively (prepared statement cache, direct handoff, FIFO scheduling).

## Transaction mode (recommended)

```yaml
pools:
  mydb:
    pool_mode: "transaction"
```

A backend connection is held for the duration of a transaction, then returned to the pool on `COMMIT`, `ROLLBACK`, or implicit completion.

This is the mode that delivers PgDoorman's connection efficiency: a `pool_size` of 40 can serve thousands of clients as long as transactions are short.

What works in transaction mode (where most poolers fail):

- Prepared statements. PgDoorman caches them per-pool, remaps statement names across backend connections, and replays preparation transparently. Drivers that pin to `unnamed` statement (Go pgx, .NET Npgsql, Python asyncpg) work without configuration.
- Pipelined batches and async `Flush` flow.
- Cancel requests over TLS.
- `LISTEN` / `NOTIFY` — but only inside a transaction; cross-transaction notifications are lost (PgBouncer same).

What does **not** work in transaction mode:

- `SET` and `RESET` outside a transaction. Use session mode for clients that rely on session-level GUC changes (`SET TIME ZONE`, `SET search_path` once per connection).
- Advisory locks held across transactions. Use session mode.
- Cursors held outside transactions (`WITH HOLD`). Use session mode.
- `SET LOCAL` works as expected — it is transaction-scoped.

## Session mode

```yaml
pools:
  legacy_app:
    pool_mode: "session"
```

A backend connection is held for the duration of the client session. Returned to the pool only when the client disconnects.

Use this when:

- The application uses session-scoped state (`SET search_path`, `SET TIME ZONE`).
- The application uses `WITH HOLD` cursors.
- The application uses advisory locks across transactions.
- You are migrating an unmodified PgBouncer deployment that was using session mode and you want a like-for-like swap.

In session mode, `pool_size` is effectively the maximum number of concurrent clients. Sizing matches PostgreSQL's `max_connections` minus reserves.

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

Returning to transaction mode in detail: when a backend goes back to the pool, PgDoorman runs `RESET ALL` and `ROLLBACK` (if `cleanup_server_connections: true`, the default). This drops:

- Session-level `SET` values.
- Cursors.
- Prepared statement names that the driver bound to specific backend names (PgDoorman's prepared statement cache survives — it is keyed by query text, not backend statement name).
- Advisory locks (`pg_advisory_unlock_all` is implicit in `RESET ALL`).

`DEALLOCATE ALL` and `DISCARD ALL` from the client also trigger PgDoorman's prepared statement cache to drop everything cached for that client. The pool-level cache is not affected.

To opt out of cleanup (for performance, in tightly-controlled deployments):

```yaml
pools:
  mydb:
    pool_mode: "transaction"
    cleanup_server_connections: false
```

Only do this if you are sure your application never leaks session state.

## Reference

- `pool_mode` parameter: [Pool Settings](../reference/pool.md#pool_mode).
- `cleanup_server_connections`: [Pool Settings](../reference/pool.md#cleanup_server_connections).
- Pool sizing: [Pool Coordinator](pool-coordinator.md), [Pool Pressure](../tutorials/pool-pressure.md).
