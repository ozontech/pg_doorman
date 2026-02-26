# Binary Upgrade Process

## Overview

PgDoorman supports seamless binary upgrades that allow you to update the software with minimal disruption to your database connections. This document explains how the upgrade process works and what to expect during an upgrade.

## Triggering a Binary Upgrade

The recommended way to trigger a binary upgrade is to send `SIGUSR2` to the PgDoorman process:

```bash
kill -USR2 $(pgrep pg_doorman)
```

Alternatively, you can use the admin console command:

```sql
UPGRADE;
```

### Signal Reference

| Signal | Behavior |
|--------|----------|
| `SIGUSR2` | Binary upgrade + graceful shutdown **(recommended)** |
| `SIGINT` | Binary upgrade + graceful shutdown (legacy, daemon/no-TTY only). In foreground mode with a TTY, SIGINT (Ctrl+C) performs graceful shutdown **without** binary upgrade. |
| `SIGTERM` | Immediate shutdown |
| `SIGHUP` | Reload configuration |

```admonish note title="Legacy SIGINT behavior"
SIGINT still triggers binary upgrade when running in daemon mode or without a TTY (e.g. when spawned by systemd). If you are running pg_doorman interactively in a terminal, Ctrl+C will cleanly stop the process without spawning a new one. Use `kill -USR2` or the `UPGRADE` admin command to trigger binary upgrade in foreground mode.
```

## How the Upgrade Process Works

When PgDoorman receives the upgrade signal:

1. The current PgDoorman instance validates the configuration of the new binary using `-t` flag
2. If validation passes, a new process is started:
   - **Daemon mode**: a new daemonized process is spawned
   - **Foreground mode**: the listener socket is passed to the new process via `--inherit-fd`
3. The new process uses the `SO_REUSE_PORT` socket option, allowing the operating system to distribute incoming traffic to the new instance
4. The old instance then closes its socket for incoming connections
5. Existing connections are handled gracefully during the transition

## systemd Integration

The recommended systemd service configuration uses `SIGUSR2` for reload:

```ini
ExecReload=/bin/kill -SIGUSR2 $MAINPID
```

This triggers a binary upgrade when you run `systemctl reload pg_doorman`.

## Handling Existing Connections

During the upgrade process, PgDoorman handles existing connections as follows:

1. Current queries and transactions are allowed to complete within the specified `shutdown_timeout` (default: 10 seconds)
2. After each query or transaction completes successfully, PgDoorman returns error code `58006` to the client
3. This error code indicates to the client that they need to reconnect to the server
4. After reconnecting, clients can safely retry their queries with the new PgDoorman instance

## Important Considerations

```admonish warning title="Query Repetition"
Repeating a query without receiving error code `58006` may cause problems as described in [this issue](https://github.com/lib/pq/issues/939). Make sure your client application properly handles reconnection scenarios.
```

```admonish tip title="Client Library Compatibility"
Be careful when using client libraries like `github.com/lib/pq` or Go's standard `database/sql` package. Ensure they properly handle the reconnection process during binary upgrades.
```
