# Binary upgrade

Update pg_doorman without dropping client connections. The old process
transfers idle clients to the new one through a Unix socket — clients
continue working without reconnecting.

## Triggering an upgrade

Send `SIGUSR2` to the pg_doorman process:

```bash
kill -USR2 $(pgrep pg_doorman)
```

Or use the admin console:

```sql
UPGRADE;
```

### Signal reference

| Signal | Behavior |
|--------|----------|
| `SIGUSR2` | Binary upgrade + graceful shutdown. **Recommended.** |
| `SIGINT` | Foreground + TTY (Ctrl+C): graceful shutdown only, no upgrade. Daemon / non-TTY: binary upgrade (legacy compatibility). |
| `SIGTERM` | Immediate exit. Active transactions are killed. |
| `SIGHUP` | Reload configuration without restart. |

```admonish note title="Legacy SIGINT behavior"
SIGINT triggers binary upgrade in daemon mode or without a TTY (e.g. when spawned by systemd). In an interactive terminal, Ctrl+C stops the process cleanly without spawning a new one. Use `kill -USR2` or the `UPGRADE` admin command for binary upgrade in foreground mode.
```

### systemd integration

```ini
[Service]
ExecReload=/bin/kill -SIGUSR2 $MAINPID
```

`systemctl reload pg_doorman` triggers a binary upgrade.

## How the upgrade works

### 1. Configuration validation

The current binary re-executes itself with the `-t` flag and the new
config. If validation fails, the upgrade is aborted — the old process
keeps serving traffic. An error banner appears in the logs.

### 2. New process startup

**Foreground mode:** a Unix socketpair is created for client migration.
The listener socket passes to the child via `--inherit-fd`. The parent
waits up to 10 seconds for a readiness signal, then closes its listener.

**Daemon mode:** a new daemon process starts. The old daemon closes its
listener. Client migration via socketpair is not used — existing clients
drain normally.

### 3. Client migration (foreground)

Idle clients (not inside a transaction) migrate to the new process:

1. pg_doorman serializes client state: `connection_id`, `secret_key`,
   pool name, username, server parameters, prepared statement cache.
2. The TCP socket is duplicated via `dup()` and passed to the new process
   through `SCM_RIGHTS`.
3. The new process reconstructs the client and assigns it to a fresh
   backend pool.

The client does not notice the migration. The TCP connection stays the
same. No reconnect, no error 58006.

### 4. Clients in transactions

A client inside `BEGIN ... COMMIT` keeps running on the old process.
After the transaction ends (COMMIT or ROLLBACK), the client migrates
on the next idle iteration.

If `shutdown_timeout` expires before the transaction finishes, the old
process force-closes the connection.

### 5. Old process exit

The shutdown timer checks the client count every 250 ms. When all
clients have migrated or disconnected, the old process exits. If
`shutdown_timeout` elapses, it exits regardless.

## Prepared statements

Each client's prepared statement cache is serialized during migration:
names, query hashes, full query text, parameter type OIDs.

In the new process:
- Entries are registered in the pool-level cache (DashMap).
- Server backends are fresh — they have no prepared statements.
- On the first `Bind` to a migrated statement, pg_doorman transparently
  sends `Parse` to the new backend. The client does not see this.

Limitations:
- If the new config has a smaller `prepared_statements_cache_size`,
  excess entries are evicted (LRU).
- Anonymous prepared statements (empty-name `Parse`) survive migration
  but require a re-`Parse` before `Bind` in the new process.

## TLS migration

By default, TLS clients drain during upgrade — their TCP socket is
passed, but the encrypted session cannot continue without key material.

The opt-in `tls-migration` feature solves this: a patched OpenSSL
exports the symmetric cipher state (keys, IVs, sequence numbers),
passes it alongside the socket, and the new process imports the state
to resume encryption. The client does not re-handshake.

### Building with TLS migration

```bash
cargo build --release --features tls-migration
```

Requires Perl and `patch` in the build environment — vendored
OpenSSL 3.5.5 compiles from source.

### Offline builds

```bash
# Download the tarball in advance
curl -fLO https://github.com/openssl/openssl/releases/download/openssl-3.5.5/openssl-3.5.5.tar.gz

# Build with the local tarball
OPENSSL_SOURCE_TARBALL=./openssl-3.5.5.tar.gz \
  cargo build --release --features tls-migration
```

SHA-256 is verified automatically in all cases.

### Restrictions

- **Linux only.** macOS and Windows use platform-native TLS (Security.framework / SChannel), not OpenSSL.
- **Same certificates.** Both processes must use the same `tls_private_key` and `tls_certificate`. The cipher state is bound to the SSL_CTX created from the certificate.
- **FIPS incompatible.** Vendored OpenSSL is not FIPS-validated. For FIPS compliance, build without `tls-migration` (system OpenSSL).
- **No HSM/PKCS#11.** Vendored OpenSSL is built with `no-engine`.

## Configuration

### `shutdown_timeout`

Maximum time to wait for in-transaction clients before force-closing.
Default: 10 seconds.

For production with long-running analytics queries: 30-60 seconds.

```toml
[general]
shutdown_timeout = 60000  # milliseconds
```

### `tls_private_key` / `tls_certificate`

For TLS migration, both processes load the same files. If the
certificate changed between versions, TLS clients receive an error
during cipher state import.

### `prepared_statements_cache_size`

The client's prepared statement cache is serialized in full. If the new
config has a smaller `client_prepared_statements_cache_size`, LRU drops
excess entries.

## Troubleshooting

### Client receives "pooler is shut down now" (58006) instead of migrating

- **Ctrl+C in foreground mode.** SIGINT in TTY = shutdown without upgrade. Use `kill -USR2`.
- **Daemon mode.** Daemon mode does not use fd-based migration — clients drain.
- **`PG_DOORMAN_CI_SHUTDOWN_ONLY=1` is set.** Forces shutdown-only mode.

### Old process does not exit

- A client is stuck in a long transaction. Wait for `shutdown_timeout` or end the transaction manually.
- Admin connections do not migrate. Close the admin session.
- Send `SIGTERM` for immediate exit: `kill -TERM <pid>`

### TLS connection dropped after upgrade

- Binary built without `--features tls-migration`. Rebuild.
- Not running on Linux. TLS migration is Linux-only.
- Certificate or key changed between versions. Use the same files.

### "TLS migration not available" in logs

The new process received a migration payload with TLS data but was
built without `--features tls-migration` or is not running on Linux.
The client is disconnected. Rebuild with the feature flag.

### "migration channel send failed" in logs

The migration channel is full (capacity: 4096). Possible when thousands
of clients migrate simultaneously. The client retries on the next idle
iteration.

```admonish warning title="Client library compatibility"
Libraries like `github.com/lib/pq` or Go's `database/sql` may need configuration to handle the reconnection path for clients that receive error 58006 (those in daemon mode or stuck past `shutdown_timeout`). See [this issue](https://github.com/lib/pq/issues/939).
```
