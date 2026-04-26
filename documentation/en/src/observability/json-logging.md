# JSON Structured Logging

PgDoorman emits structured JSON logs when run with `--log-format Structured`. Each line is a self-contained JSON object with timestamp, level, source location, and message — ready for ingestion into Loki, Elasticsearch, Datadog, or any log pipeline that expects JSON.

## Enabling

Three equivalent ways:

```bash
# Command line flag
pg_doorman -F Structured /etc/pg_doorman/pg_doorman.yaml

# Long form
pg_doorman --log-format Structured /etc/pg_doorman/pg_doorman.yaml

# Environment variable
LOG_FORMAT=Structured pg_doorman /etc/pg_doorman/pg_doorman.yaml
```

The default is `Text` (human-readable). The `--log-format` flag accepts `Text`, `Structured`, or `Debug`; the last is currently an alias for `Text`.

## Output

```json
{"timestamp":"2026-04-25T08:32:14.512Z","level":"INFO","file":"src/app/server.rs","line":357,"message":"Server is up at 0.0.0.0:6432"}
{"timestamp":"2026-04-25T08:32:14.514Z","level":"INFO","file":"src/pool/mod.rs","line":421,"message":"Pool 'mydb' initialized: 1 user, pool_size=40"}
{"timestamp":"2026-04-25T08:32:18.103Z","level":"WARN","file":"src/server/protocol_io.rs","line":189,"message":"Backend connection lost: connection reset by peer"}
```

Fields:

| Field | Type | Notes |
| --- | --- | --- |
| `timestamp` | RFC 3339 string | UTC, millisecond precision. |
| `level` | string | `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`. |
| `file` | string | Source file emitting the log. |
| `line` | integer | Line number. |
| `message` | string | Human-readable message. |

There are no nested fields or per-event labels — PgDoorman's logger is plain `log` macro events serialized to JSON. For richer metadata (per-pool counters, per-client events), use Prometheus metrics instead. See [Prometheus reference](../reference/prometheus.md).

## Log level

Set via `general.log_level` in the config or override at startup:

```yaml
general:
  log_level: "info"
```

```bash
pg_doorman -l debug -F Structured /etc/pg_doorman/pg_doorman.yaml
```

Change at runtime via the admin database:

```sql
SET log_level = 'debug';
SHOW LOG_LEVEL;
```

This affects the running process only. Persisting requires editing the config and `RELOAD`/`SIGHUP`.

## Recommended pipeline

For Kubernetes:

```yaml
spec:
  containers:
    - name: pg_doorman
      image: ghcr.io/ozontech/pg_doorman:latest
      args:
        - "-F"
        - "Structured"
        - "/etc/pg_doorman/pg_doorman.yaml"
      env:
        - name: LOG_LEVEL
          value: "info"
```

Logs go to stdout, container runtime captures them, your log shipper (Promtail, Fluent Bit, Vector) forwards as-is — JSON is preserved end to end.

For systemd:

```ini
[Service]
ExecStart=/usr/bin/pg_doorman -F Structured /etc/pg_doorman/pg_doorman.yaml
StandardOutput=journal
StandardError=journal
```

`journalctl -u pg_doorman -o json` gives you the JSON back.

## Caveats

- For production, choose `Text` (terminals, syslog) or `Structured` (log shippers). `Debug` is reserved for future use and currently equals `Text`.
- Source `file` and `line` come from `log` macro call sites. They survive in release builds because PgDoorman ships with debug info enabled.
- The logger does not include trace IDs or request correlation. For per-request tracing, use `SHOW CLIENTS` and Prometheus metrics.

## Where to next

- [Prometheus reference](../reference/prometheus.md) — for machine-readable metrics.
- [Latency Percentiles](percentiles.md) — for performance signals.
- [Admin Commands](admin-commands.md) — for runtime introspection.
