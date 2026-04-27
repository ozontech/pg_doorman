# Troubleshooting

Symptoms you are likely to hit during the first week of running PgDoorman, and what to look at when you do.

## Authentication errors when connecting to PostgreSQL

**Symptom:** PgDoorman accepts the client connection, but the first query returns `password authentication failed` from PostgreSQL.

### The pool username matches the backend role

PgDoorman uses **passthrough authentication** by default — the cryptographic proof the client sent (MD5 hash or SCRAM `ClientKey`) is reused to authenticate against PostgreSQL. The `password` field in your config must hold the exact hash from `pg_authid` / `pg_shadow`:

```sql
SELECT usename, passwd FROM pg_shadow WHERE usename = 'your_user';
```

For SCRAM, both processes must see the **same** salt and iteration count — even a one-character difference in the stored verifier breaks passthrough.

### The pool username differs from the backend role

When the client-facing `username` in PgDoorman does not match the actual PostgreSQL role, passthrough cannot work — there is nothing to pass through. Provide explicit credentials:

```yaml
users:
  - username: "app_user"              # client-facing name
    password: "md5..."                # hash for client → pg_doorman auth
    server_username: "pg_app_user"    # actual PostgreSQL role
    server_password: "plaintext_pwd"  # plaintext password for that role
    pool_size: 40
```

This is also the path for JWT auth, where the client never sends a password and there is nothing to pass through.

```admonish tip title="Where to get the password hash"
`pg_doorman generate --host …` introspects PostgreSQL and emits a config with the hashes already filled in. Faster than copy-pasting from `pg_shadow`.
```

## Configuration file not found

**Symptom:** PgDoorman exits with `configuration file not found` on startup.

By default the binary looks for `pg_doorman.toml` in the current working directory. Either name your file that way and `cd` to its directory, or pass the path explicitly:

```bash
pg_doorman /etc/pg_doorman/pg_doorman.yaml
```

Validate before starting:

```bash
pg_doorman -t /etc/pg_doorman/pg_doorman.yaml
```

## Clients receive `58006` (`pooler is shut down now`)

The pool is shutting down or the binary upgrade was issued in daemon mode. Check the server logs around the timestamp of the error:

- `Got SIGUSR2, starting binary upgrade …` — a binary upgrade is in progress. In foreground mode, idle clients should migrate transparently; only clients still inside a transaction past `shutdown_timeout` get `58006`. In daemon mode there is no fd-based migration and every client gets `58006` when its connection is closed. See [Binary upgrade → Troubleshooting](binary-upgrade.md#troubleshooting).
- No `SIGUSR2` log line — someone sent `SIGTERM` or `SIGINT` and the pooler shut down without spawning a successor. Check the systemd unit, the pid in question, and your operator runbook.

If the `58006` happened during a planned upgrade, this is expected for that subset of clients. Configure the application's connection pool to retry on transient errors.

## Pool size too small

**Symptom:** Queries take much longer end-to-end than they do when run directly against PostgreSQL.

Look at `SHOW POOLS` and `SHOW POOLS_EXTENDED`:

```
cl_waiting   — how many clients are queued for a backend right now
maxwait      — longest time any waiter has been queued, in seconds
sv_idle      — idle backends in the pool
sv_active    — backends currently checked out
```

If `cl_waiting > 0` consistently and `sv_idle == 0`, the pool is undersized for the load. Either raise `pool_size` for that user, or look at why `sv_active` stays high — long transactions, idle-in-transaction sessions, or a slow downstream call holding the backend.

If you are also using `max_db_connections`, watch `SHOW POOL_COORDINATOR` for `evictions` (donors are giving up connections under pressure) and `exhaustions` (the cap was hit even after evictions). See [Pool Coordinator](../concepts/pool-coordinator.md).

## Where to file what is left

```admonish tip title="Still stuck?"
If your problem isn't here, [open an issue on GitHub](https://github.com/ozontech/pg_doorman/issues) with: pg_doorman version, the relevant config (passwords redacted), the client driver and version, and the matching log lines from both pg_doorman and PostgreSQL.
```
