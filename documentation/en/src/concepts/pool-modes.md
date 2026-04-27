# Pool Modes

PgDoorman supports two pool modes: `transaction` and `session`. Set per pool, with optional per-user override.

There is no `statement` mode. Statement pooling rotates the backend after every statement, which forces clients to give up multi-statement transactions and breaks the prepared-statement protocol entirely; PgDoorman invests its tuning (prepared-statement cache, direct handoff, strict-FIFO scheduling) in transaction mode instead. PgBouncer keeps `statement` mode for backward compatibility; Odyssey omits it.

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
- `LISTEN` / `NOTIFY` — but only inside a transaction. A `LISTEN` issued and then committed releases the backend, and any notifications delivered to it after that go to whichever client checks it out next, not to the original `LISTEN`-er. PgBouncer behaves the same way; if you need cross-transaction `LISTEN`, use session mode for that client.

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

Cleanup in transaction mode is **mutation-tracked**, not unconditional. PgDoorman watches each transaction for `SET`, `PREPARE`, and `DECLARE CURSOR`, and only when the backend returns to the pool with one of those flags set does it issue `RESET ALL`, `DEALLOCATE ALL`, or `CLOSE ALL` respectively. A read-only transaction skips cleanup entirely — that's a measurable win on hot OLTP paths.

What gets reset when a flag fires:

- `SET` flag → `RESET ALL` drops session-level GUCs and runs `pg_advisory_unlock_all` implicitly.
- `PREPARE` flag → `DEALLOCATE ALL` drops PostgreSQL-side prepared statements that the driver named explicitly. PgDoorman's own prepared-statement cache survives the reset because it is keyed by query text, not by backend name.
- `DECLARE CURSOR` flag → `CLOSE ALL` drops cursors.

`DEALLOCATE ALL` and `DISCARD ALL` issued by the client clear that client's prepared-statement cache (so the next `Parse` registers anew). The pool-level shared cache is not affected; other clients keep their entries.

To opt out of cleanup entirely (for performance, in tightly-controlled deployments):

```yaml
pools:
  mydb:
    pool_mode: "transaction"
    cleanup_server_connections: false
```

Only do this if you are sure your application never leaks session state. The mutation-tracked default is already cheap when no mutation happened, so the opt-out is rarely worth the risk.

## Reference

- `pool_mode` parameter: [Pool Settings](../reference/pool.md#pool_mode).
- `cleanup_server_connections`: [Pool Settings](../reference/pool.md#cleanup_server_connections).
- Pool sizing: [Pool Coordinator](pool-coordinator.md), [Pool Pressure](../tutorials/pool-pressure.md).
