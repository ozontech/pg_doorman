# Signals and Reload

PgDoorman responds to four POSIX signals: `SIGHUP`, `SIGINT`, `SIGUSR2`, and `SIGTERM`. Each does one specific thing.

## Quick reference

| Signal | Effect | Existing connections | When to use |
| --- | --- | --- | --- |
| `SIGHUP` | Reload config from disk. | Preserved. | Adjust pools, rotate server TLS certs, edit `pg_hba.conf`. |
| `SIGTERM` | Graceful shutdown. | Drained until idle, then closed. | Stopping the service. |
| `SIGUSR2` | Graceful binary upgrade. | Migrated to a new process. | Replacing the binary without downtime. |
| `SIGINT` | Depends on TTY (see below). | Varies. | Ctrl+C in development; deprecated in production. |

## Reload (`SIGHUP`)

```bash
kill -HUP $(pidof pg_doorman)
```

Re-reads the config file and applies changes. What reloads:

- Pool definitions (added, removed, resized).
- User lists, passwords, `auth_query` blocks.
- `pg_hba.conf` rules (file or inline content).
- Server-side TLS certificates and CA bundles (lock-free swap; existing TLS connections keep their original context).
- Talos and JWT public keys.
- Log level and format.

What does **not** reload:

- `general.host`, `general.port` — listening socket is fixed at startup.
- Client-side TLS certificates — process restart required, or use binary upgrade.
- Worker thread count and Tokio runtime parameters.

After reload, `SHOW CONFIG` reflects the new values. Existing client connections are not re-evaluated against the new `pg_hba.conf` — only new connections.

## Graceful shutdown (`SIGTERM`)

```bash
kill -TERM $(pidof pg_doorman)
```

PgDoorman:

1. Stops accepting new connections.
2. Closes idle backend connections.
3. Logs how many clients are still in transactions.
4. Waits up to `shutdown_timeout` (default 10s) for clients to finish.
5. Exits.

`shutdown_timeout` is a hard cap. Clients still in transactions when it fires receive a connection close.

```yaml
general:
  shutdown_timeout: "30s"
```

For systemd, set `TimeoutStopSec` to a value larger than `shutdown_timeout` so systemd does not `SIGKILL` while PgDoorman is still draining.

## Binary upgrade (`SIGUSR2`)

```bash
kill -USR2 $(pidof pg_doorman)
```

The recommended way to replace the binary without dropping clients:

1. Replace the binary on disk with the new version.
2. Send `SIGUSR2` to the running process.
3. The current process spawns a child running the new binary, hands over the listening socket, and continues serving existing clients until they finish.
4. New clients connect to the child immediately.
5. The old process exits when the last client transaction completes (or on `shutdown_timeout`).

The child sends `sd_notify MAINPID=<new_pid>` so systemd `Type=notify` units update their main PID seamlessly.

For the full protocol, TLS migration, and rollback, see [Binary Upgrade](../tutorials/binary-upgrade.md).

## `SIGINT` (Ctrl+C)

`SIGINT` is context-sensitive:

- **Foreground with a TTY** (development, `cargo run`): graceful shutdown only.
- **Daemon mode or no TTY** (legacy production): triggers a binary upgrade + graceful shutdown, like `SIGUSR2`.

The legacy `SIGINT` upgrade path exists for backward compatibility with deployments that send `SIGINT` from init scripts. New deployments should use `SIGUSR2` for upgrade and `SIGTERM` for shutdown explicitly.

## systemd integration

PgDoorman supports `Type=notify`. The shipped `pg_doorman.service` unit runs the binary in the foreground and notifies systemd via `sd_notify`:

```ini
[Service]
Type=notify
ExecStart=/usr/bin/pg_doorman /etc/pg_doorman/pg_doorman.yaml
ExecReload=/bin/kill -HUP $MAINPID
KillSignal=SIGTERM
TimeoutStopSec=60
Restart=on-failure
```

`sd_notify READY=1` is sent after the listening socket is bound and pools are initialized. `sd_notify MAINPID=<child>` is sent during binary upgrade so systemd tracks the new process correctly.

If you migrate from `Type=forking` + `--daemon`, drop `--daemon` and switch to `Type=notify` — fewer moving parts and proper readiness tracking. Older deployments using `--daemon` continue to work but do not benefit from `sd_notify`.

## Daemon mode

`pg_doorman --daemon` forks into the background and writes its PID to `daemon_pid_file` (default `/tmp/pg_doorman.pid`). For systemd users, prefer `Type=notify` over `--daemon`.

```yaml
general:
  daemon_pid_file: "/var/run/pg_doorman.pid"
```

## Where to next

- [Binary Upgrade](../tutorials/binary-upgrade.md) — full upgrade protocol with TLS migration.
- [Troubleshooting](../tutorials/troubleshooting.md) — what to check when reload does not pick up changes.
- [TLS](../guides/tls.md#hot-reload) — `SIGHUP` reload semantics for server-side certificates.
