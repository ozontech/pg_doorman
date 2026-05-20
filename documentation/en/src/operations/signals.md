# Signals and Reload

PgDoorman responds to four POSIX signals: `SIGHUP`, `SIGINT`, `SIGUSR2`, and `SIGTERM`. Each does one specific thing.

## Quick reference

| Signal | Effect | Existing connections | When to use |
| --- | --- | --- | --- |
| `SIGHUP` | Reload config from disk. | Preserved. | Adjust pools, rotate server TLS certs, edit `pg_hba.conf`. |
| `SIGTERM` | Immediate shutdown. | Closed. | Stopping the service when reconnects are acceptable. |
| `SIGUSR2` | Binary upgrade and old-process drain. | Migrated to a new process where possible. | Replacing the binary without downtime. |
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
- `general.tcp_socket_buffer_size` on existing sockets — the new value
  is applied only when pg_doorman accepts a new client TCP socket or
  opens a new backend TCP socket.
- Client-facing TLS certificates — process restart required. Do not rotate
  them during an upgrade where TLS session migration is required.
- Worker thread count and Tokio runtime parameters.

After reload, `SHOW CONFIG` reflects the new values. Existing client connections are not re-evaluated against the new `pg_hba.conf` — only new connections. Existing TCP sockets also keep the socket buffer size that was applied when the socket was created.

## Immediate shutdown (`SIGTERM`)

```bash
kill -TERM $(pidof pg_doorman)
```

pg_doorman logs how many clients are still in transactions and exits.
It does not wait for `shutdown_timeout` and it does not migrate active
transactions. All client connections are closed by process exit.

`shutdown_timeout` applies to `SIGUSR2` binary upgrade drain, not to
plain `SIGTERM` shutdown.

## Binary upgrade (`SIGUSR2`)

```bash
kill -USR2 $(pidof pg_doorman)
```

The recommended way to replace the binary without dropping clients:

1. Replace the binary on disk with the new version using an atomic rename.
2. Send `SIGUSR2` to the running process.
3. The current process validates the new binary with `-t`.
4. The current process spawns a child running the new binary, hands over the listening socket, and continues serving existing clients until they finish.
5. New clients connect to the child immediately.
6. The old process exits when the last client transaction completes (or on `shutdown_timeout`).

The child sends `sd_notify MAINPID=<new_pid>` so systemd `Type=notify`
units track the new main PID correctly.

Migrated client TCP sockets are configured again in the child process, so
a changed `general.tcp_socket_buffer_size` applies to those clients during
binary upgrade. Backend TCP sockets are opened by the new process and use
the new value when they connect.

For the full protocol, TLS migration, and rollback, see [Binary Upgrade](../tutorials/binary-upgrade.md).

## `SIGINT` (Ctrl+C)

`SIGINT` is context-sensitive:

- **Foreground with a TTY** (development, `cargo run`): shutdown only.
- **Daemon mode or no TTY** (legacy production): triggers binary upgrade
  and old-process drain, like `SIGUSR2`.

The legacy `SIGINT` upgrade path exists for backward compatibility with deployments that send `SIGINT` from init scripts. New deployments should use `SIGUSR2` for upgrade and `SIGTERM` for shutdown explicitly.

## systemd integration

PgDoorman supports `Type=notify`. The shipped `pg_doorman.service` unit runs the binary in the foreground and notifies systemd via `sd_notify`:

```ini
[Service]
Type=notify
NotifyAccess=exec
ExecStart=/usr/bin/pg_doorman /etc/pg_doorman/pg_doorman.toml
ExecReload=/bin/kill -SIGUSR2 $MAINPID
ExecStop=/bin/kill -SIGTERM $MAINPID
SyslogIdentifier=pg_doorman
KillMode=mixed
TimeoutStopSec=60
Restart=on-failure
Nice=-15
User=postgres
Group=postgres
LimitNOFILE=65536
```

`sd_notify READY=1` is sent after the listening socket is bound and pools are initialized. `sd_notify MAINPID=<child>` is sent during binary upgrade so systemd tracks the new process correctly.

With this unit, `systemctl reload pg_doorman` means binary upgrade
(`SIGUSR2`), not config reload (`SIGHUP`). Use `kill -HUP <pid>` when
you only need to reload configuration.

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
- [TLS](../guides/tls.md) — `SIGHUP` reload semantics for server-side certificates.
