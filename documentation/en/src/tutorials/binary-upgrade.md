# Binary upgrade

Update pg_doorman without dropping client connections. The old process
transfers idle clients to the new one through a Unix socket -- clients
continue working on the same TCP connection without reconnecting.

## Quick start

```bash
# 1. Replace the binary on disk (build, download, etc.)
cp pg_doorman_new /usr/bin/pg_doorman

# 2. Trigger upgrade
kill -USR2 $(pgrep pg_doorman)

# 3. Verify: old PID gone, clients still connected
pgrep pg_doorman   # new PID
```

Or via the admin console:

```sql
UPGRADE;
```

## How the upgrade works

```
                        SIGUSR2
                           |
                           v
               +-----------------------+
               | 1. Validate config    |
               |    (pg_doorman -t)    |   -- fail --> abort, keep serving
               +-----------+-----------+
                           |
                           v
               +-----------------------+
               | 2. Spawn new process  |
               |    socketpair()       |
               |    inherit-fd         |
               |    readiness pipe     |   -- wait up to 10s
               +-----------+-----------+
                           |
             +-------------+-------------+
             |                           |
             v                           v
  +---------------------+    +---------------------+
  | OLD process         |    | NEW process         |
  |                     |    |                     |
  | 3. Idle clients     |    | migration_receiver  |
  |    serialize state  +--->+    reconstruct      |
  |    dup() + SCM_RIGHTS    |    spawn client     |
  |                     |    |    handle()         |
  | 4. In-tx clients    |    |                     |
  |    finish tx        |    | Accepts new conns   |
  |    migrate on idle  +--->+                     |
  |                     |    |                     |
  | 5. Shutdown timer   |    +---------------------+
  |    poll 250ms       |
  |    exit when empty  |
  +---------------------+
```

### Phase 1: Config validation

The current binary re-executes itself with `-t` and the config file.
If validation fails, the upgrade is aborted. The old process keeps
serving traffic. An error banner appears in the logs:

```
!!!  BINARY UPGRADE ABORTED - SHUTDOWN CANCELLED  !!!
!!!  FIX THE CONFIGURATION BEFORE ATTEMPTING BINARY UPGRADE AGAIN  !!!
!!!  THE SERVER WILL CONTINUE RUNNING WITH THE CURRENT BINARY  !!!
```

### Phase 2: Spawn new process

**Foreground mode:**

1. A Unix `socketpair()` is created for client migration.
2. The listener fd passes to the child via `--inherit-fd`.
3. A pipe signals readiness: the parent waits up to 10 seconds for
   a single byte. If the child starts and begins accepting, it
   writes to the pipe.
4. The parent closes its listener -- new connections go to the child.

**Daemon mode:**

A new daemon process starts. The old daemon closes its listener.
Client migration via socketpair is not used -- existing clients
drain normally (receive error 58006 when `shutdown_timeout` expires).

### Phase 3: Idle client migration (foreground)

When `MIGRATION_IN_PROGRESS` is set, each idle client (not in a
transaction, no pending deferred `BEGIN`, no buffered reads)
migrates:

1. **Serialize**: connection_id, secret_key, pool name, username,
   server parameters, full prepared statement cache.
2. **dup() + SCM_RIGHTS**: the TCP socket fd is duplicated and
   sent to the new process over the Unix socketpair.
3. **Reconstruct**: the new process rebuilds the Client struct,
   assigns it to the correct pool, and calls `handle()`.

The client sees no interruption. No reconnect, no error, no
re-authentication. The TCP connection is the same physical socket.

### Phase 4: In-transaction client drain

A client inside `BEGIN ... COMMIT` continues running on the old
process. Its server connection stays alive. After the transaction
ends (COMMIT or ROLLBACK), the client becomes idle and migrates
on the next loop iteration.

A deferred `BEGIN` (no server checked out yet) also blocks migration.
The client must send a query (flushing the deferred BEGIN) and then
COMMIT before it can migrate.

### Phase 5: Shutdown timer

The shutdown timer polls `CURRENT_CLIENT_COUNT` every 250 ms. When
all clients have migrated or disconnected, the old process calls
`process::exit(0)`.

If `shutdown_timeout` elapses before all clients finish, the old
process exits regardless -- force-closing remaining connections.

During migration, `drain_all_pools()` is deferred. In-transaction
clients still need their server connections. Pool draining starts
only after migration completes or when `MIGRATION_IN_PROGRESS`
is cleared.

## Prepared statements

Each client's prepared statement cache is serialized during migration:

- Statement key (named or anonymous hash)
- Query hash
- Full query text
- Parameter type OIDs

In the new process:

1. Each entry is registered in the pool-level shared cache (DashMap).
2. Server backends are fresh -- they have no prepared statements.
3. On the first `Bind` to a migrated statement, pg_doorman
   transparently sends `Parse` to the new backend. The client does
   not see this extra round-trip.

**Limits:**

- If the new config has a smaller `client_prepared_statements_cache_size`,
  excess entries are evicted (LRU). The remaining entries work normally.
- Anonymous prepared statements (empty-name `Parse`) survive migration
  but require a re-`Parse` before `Bind` in the new process.
- `DEALLOCATE ALL` after migration clears the transferred cache. Re-`Parse`
  with the same name uses the new query text.

## TLS migration

By default, TLS clients cannot be migrated -- the encrypted session
requires key material that lives inside the OpenSSL state machine.
These clients drain during upgrade: their connection is closed when
`shutdown_timeout` expires, and the client reconnects to the new
process.

The opt-in `tls-migration` feature solves this. A patched OpenSSL
exports the symmetric cipher state, passes it alongside the fd over
the Unix socket, and the new process imports it to resume encryption
mid-stream. The client does not re-handshake.

### What gets exported

The patch adds `SSL_export_migration_state()` and
`SSL_import_migration_state()` to OpenSSL 3.5.5. Exported data:

- TLS protocol version
- Cipher suite ID and tag length
- Read/write symmetric keys (AES key schedule input, not expanded)
- Read/write IVs (nonce)
- Read/write sequence numbers (8 bytes each)
- For TLS 1.3: server and client application traffic secrets

This is enough to reconstruct the record layer in the new process
and continue encrypting/decrypting on the same TCP connection.

### Building with TLS migration

```bash
cargo build --release --features tls-migration
```

Requires `perl` and `patch` in the build environment. Vendored
OpenSSL 3.5.5 compiles from source with the migration patch applied.

### Offline builds

```bash
# Download the tarball in advance
curl -fLO https://github.com/openssl/openssl/releases/download/openssl-3.5.5/openssl-3.5.5.tar.gz

# Build with the local tarball
OPENSSL_SOURCE_TARBALL=./openssl-3.5.5.tar.gz \
  cargo build --release --features tls-migration
```

SHA-256 is verified automatically.

### Restrictions

- **Linux only.** macOS and Windows use platform-native TLS
  (Security.framework / SChannel), not OpenSSL. TLS migration is
  not possible with native-tls backends.
- **Same certificates.** Both processes must use the same
  `tls_private_key` and `tls_certificate`. The cipher state is bound
  to the SSL_CTX created from the certificate. Changed certificates
  cause import failure and client disconnection.
- **FIPS incompatible.** Vendored OpenSSL is not FIPS-validated.
  For FIPS compliance, build without `tls-migration` (TLS clients
  drain instead of migrating).
- **No HSM/PKCS#11.** Vendored OpenSSL is built with `no-engine`.

### Known limitations

- **KeyUpdate (TLS 1.3) not supported.** If a client or pg_doorman
  sends a KeyUpdate message, the exported keys become stale. In
  practice, libpq and PostgreSQL do not send KeyUpdate. Custom
  clients with aggressive key rotation may be affected.
- **SSL_pending data not checked.** The migration happens at the
  idle point, where no application data is buffered. The idle-point
  invariant guarantees this, but there is no explicit SSL_pending()
  assertion.
- **Tied to OpenSSL 3.5.5.** The patch modifies internal OpenSSL
  structures (`ssl_local.h`, `rec_layer_s3.c`, `ssl_lib.c`).
  Upgrading OpenSSL requires reviewing and re-applying the patch
  against the new version.

## Signal reference

| Signal | Behavior |
|--------|----------|
| `SIGUSR2` | Binary upgrade + graceful shutdown. **Recommended for all modes.** |
| `SIGINT` | Foreground + TTY (Ctrl+C): graceful shutdown only, no upgrade. Daemon / non-TTY: binary upgrade (legacy compatibility). |
| `SIGTERM` | Immediate exit. Active transactions are killed. All clients disconnected. |
| `SIGHUP` | Reload configuration without restart. No downtime. |
| `UPGRADE` (admin) | Sends SIGUSR2 to the current process internally. Same effect. |

```admonish note title="Legacy SIGINT behavior"
SIGINT triggers binary upgrade in daemon mode or without a TTY (e.g. when spawned by systemd). In an interactive terminal, Ctrl+C stops the process cleanly without spawning a new one. Use `kill -USR2` or the `UPGRADE` admin command for binary upgrade in foreground mode.
```

## Daemon vs foreground

| | Foreground | Daemon |
|---|---|---|
| Client migration via fd passing | Yes (socketpair) | No |
| Idle clients preserved | Yes | No (drain with 58006) |
| In-tx clients | Finish tx, then migrate | Finish tx, then 58006 |
| New process startup | Inherits listener fd | Starts independently |
| Recommended for | systemd, containers, k8s | Legacy deployments |

For zero-downtime upgrades with client migration, run in foreground
mode. systemd manages the process lifecycle:

```ini
[Service]
Type=simple
ExecStart=/usr/bin/pg_doorman /etc/pg_doorman.yaml
ExecReload=/bin/kill -SIGUSR2 $MAINPID
```

## Configuration

### `shutdown_timeout`

Maximum time to wait for in-transaction clients before force-closing
connections. The old process exits after this timeout regardless of
remaining clients.

Default: 10 seconds.

For production with long-running analytics queries: 30-60 seconds.

```toml
[general]
shutdown_timeout = 60000  # milliseconds
```

Setting it too low risks killing active transactions. Setting it too
high delays the old process exit when a client is stuck (e.g.,
idle-in-transaction). Choose a value that covers your longest expected
transaction, plus margin.

### `tls_private_key` / `tls_certificate`

For TLS migration, both the old and new process load the same files.
If the certificate changed between binary versions, TLS clients
receive an error during cipher state import and are disconnected.

Rotate certificates via `SIGHUP` (config reload) first, then do
the binary upgrade.

### `prepared_statements_cache_size`

Pool-level prepared statement cache. Does not directly affect
migration, but the pool cache in the new process must be large
enough to hold entries registered by migrated clients.

### `client_prepared_statements_cache_size`

Per-client prepared statement cache. The client's cache is
serialized in full during migration. If the new config has a
smaller value, LRU eviction drops excess entries.

## Monitoring

### Logs

Key log lines during migration:

```
INFO  Got SIGUSR2, starting binary upgrade and graceful shutdown
INFO  Validating configuration with: /usr/bin/pg_doorman -t pg_doorman.yaml
INFO  Configuration validation successful
INFO  Starting new process with inherited listener fd=5
INFO  New process signaled readiness
INFO  Client migration enabled
INFO  [user@pool #c42] client 10.0.0.1:51234 migrated to new process
INFO  waiting for 3 clients in transactions
INFO  All clients disconnected, shutting down
INFO  Migration sender finished
```

In the new process:

```
INFO  migration receiver: listening for migrated clients
INFO  [user@pool #c42] migrated client accepted from 10.0.0.1:51234
INFO  migration receiver done: migration socket closed
INFO  migration receiver: stopped
```

### Prometheus metrics

| Metric | Relevance during upgrade |
|--------|--------------------------|
| `pg_doorman_pools_clients{status="active"}` | Should drop to 0 on old process |
| `pg_doorman_pools_clients{status="idle"}` | Drops as clients migrate |
| `pg_doorman_connection_count{type="total"}` | Old: decreasing, new: increasing |
| `pg_doorman_clients_prepared_cache_entries` | Confirms cache transferred |

### Admin console

```sql
-- On the new process (old rejects non-admin connections)
SHOW POOLS;
SHOW CLIENTS;
```

## Troubleshooting

### Client receives "pooler is shut down now" (58006) instead of migrating

**Ctrl+C in foreground mode.** SIGINT in TTY = shutdown without
upgrade. Use `kill -USR2` or the `UPGRADE` admin command.

**Daemon mode.** Daemon mode does not use fd-based migration.
Clients drain normally. Switch to foreground mode for migration.

**`PG_DOORMAN_CI_SHUTDOWN_ONLY=1` is set.** This env var forces
shutdown-only mode (used in CI tests). Unset it.

### Old process does not exit

**Long transaction.** A client is stuck in `BEGIN` without `COMMIT`.
Wait for `shutdown_timeout` or end the transaction manually.

**Admin connections.** Admin connections do not migrate. Close the
admin session on the old process.

**Force exit:** `kill -TERM <old_pid>` sends SIGTERM for immediate
exit.

### TLS connection dropped after upgrade

**Binary built without `--features tls-migration`.** TLS clients
drain instead of migrating. Rebuild with the feature flag.

**Not running on Linux.** TLS migration is Linux-only.

**Certificate or key changed.** The old process exported cipher state
bound to the old certificate. Use the same files for both processes.
Rotate certificates via SIGHUP before the binary upgrade.

### "TLS migration not available" in logs

The new process received a migration payload with TLS data but was
built without `--features tls-migration` or is not running on Linux.
The client is disconnected. Rebuild the new binary with the feature
flag.

### "migration channel not ready" in logs

The `MIGRATION_TX` channel has not been initialized yet. This can
happen if the new process has not finished starting when a client
tries to migrate. The client retries on the next idle iteration
(within milliseconds).

### "migration channel send failed" in logs

The migration channel is full (capacity: 4096). Possible when
thousands of clients migrate simultaneously. The client retries
on the next idle iteration.

### "prepare_migration failed" in logs

The client's raw fd is unavailable or `dup()` failed. Possible
causes: fd exhaustion, or the client connected through a code
path that does not store the raw fd. Check `ulimit -n`.

```admonish warning title="Client library compatibility"
Libraries like `github.com/lib/pq` or Go's `database/sql` may need configuration to handle the reconnection path for clients that receive error 58006 (those in daemon mode or stuck past `shutdown_timeout`). See [this issue](https://github.com/lib/pq/issues/939).
```

## Operational checklist

Before rolling out binary upgrade to production:

- [ ] Run in **foreground mode** (not daemon) for fd-based migration
- [ ] Set `shutdown_timeout` to cover your longest expected transaction
      (recommendation: 30-60 seconds for OLTP, longer for analytics)
- [ ] If using TLS: build with `--features tls-migration`, verify
      both processes use the same certificate and key files
- [ ] Test the upgrade in staging: open a session, trigger SIGUSR2,
      verify the session continues working
- [ ] Verify systemd unit has `ExecReload=/bin/kill -SIGUSR2 $MAINPID`
- [ ] Monitor logs for migration errors after the first production
      upgrade
- [ ] Confirm old process exits (check PID file or `pgrep`)
- [ ] Verify Prometheus metrics show clients on the new process
