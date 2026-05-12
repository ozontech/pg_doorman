# PostgreSQL startup parameters

Use `startup_parameters` when a pool needs PostgreSQL GUC defaults that
should be set at backend startup, without changing `postgresql.conf`,
`ALTER ROLE`, or `ALTER DATABASE`.

- A hot OLTP pool is affected by a sticky generic plan after the
  `plan_cache_mode = auto` heuristic flips. Switching the whole role
  to `force_custom_plan` would affect every workload using that role;
  setting it on one pool limits the blast radius.
- An application that does not set its own `statement_timeout` or
  `idle_in_transaction_session_timeout` and cannot be patched fast
  enough. The DBA needs a server-side default that survives the
  application's own session resets.
- A single application that should announce a stable
  `application_name` regardless of what the connecting driver
  negotiates, so `pg_stat_activity` and audit logs stay legible.

## Configuration

The cascade has three levels; the more specific level wins per key:

```toml
[general.startup_parameters]
statement_timeout = "5s"

[pools.checkout.startup_parameters]
plan_cache_mode = "force_custom_plan"
work_mem        = "64MB"
```

After `SIGHUP` (or `RELOAD` on the admin console) every new backend
for the `checkout` pool starts with `statement_timeout = 5s`,
`plan_cache_mode = force_custom_plan`, and `work_mem = 64MB`. Other
pools keep `statement_timeout = 5s` from `general` and the PG default
for the rest. Already-open backends are not affected; the change takes
hold as the pool rotates connections.

When `auth_query` runs in passthrough mode (no `server_user`), the
lookup SQL may return an optional `startup_parameters` text column
holding a JSON object. Values from that column override both
`general` and per-pool settings for that user only:

```sql
SELECT
  rolpassword AS passwd,
  CASE rolname
    WHEN 'vip' THEN '{"work_mem":"256MB"}'::text
    ELSE NULL::text
  END AS startup_parameters
FROM pg_authid
WHERE rolname = $1;
```

The column must serialise as `text`. If the SQL returns `json` or
`jsonb`, add an explicit `::text` cast. pg_doorman reads the column
as `text` and logs a one-time warning per user when the type does
not match.

Dedicated `auth_query` mode (`server_user` set) ignores the per-user
column and logs once per (pool, username): one shared backend serves
many users, so a per-user override cannot apply.

## What pg_doorman does with the values

The merged map is written into the PostgreSQL `StartupMessage` of
every backend pg_doorman opens. PG records each entry as the session
default for that setting (`pg_settings.reset_val` and
`pg_settings.source = 'client'`), so client-side `RESET ALL` and
`DISCARD ALL` restore the operator value rather than discarding it.
Operators get a stable session default without editing
`postgresql.conf` or running `ALTER ROLE`.

The values can be observed from the client:

```text
checkout=> SHOW plan_cache_mode;
 plan_cache_mode
-------------------
 force_custom_plan

checkout=> SET plan_cache_mode = 'auto'; RESET ALL; SHOW plan_cache_mode;
 plan_cache_mode
-------------------
 force_custom_plan
```

## Validation

At config load:

- Keys must match PG GUC naming `^[A-Za-z_][A-Za-z0-9_.]*$`. Namespaced
  names like `auto_explain.log_min_duration` are accepted; arbitrary
  punctuation is not.
- Reserved keys (`user`, `database`, `replication`, `options`, and
  anything starting with `_pq_.`) are refused. pg_doorman manages
  them itself or PG treats them specially in the StartupMessage.
- Values must not contain null bytes.
- Each level (general or per-pool) must fit within the operator
  budget: `MAX_STARTUP_PACKET_LENGTH` (10 000 bytes) minus 512 bytes
  reserved for pg_doorman-managed keys.

At every backend spawn pg_doorman re-checks the merged cascade
against the same cap. Two levels that fit individually can together
push past it once `auth_query` adds a third layer; when that happens
pg_doorman drops every operator-supplied key for that one spawn,
logs the byte counts, and starts the backend with PostgreSQL defaults
for those keys.

## What happens when PG rejects a parameter

If PostgreSQL rejects an operator-supplied parameter at backend
startup, pg_doorman forwards the PG `ErrorResponse` to the client
unchanged. The client sees the same sqlstate (`22023`,
`42704`, `42501`, `55P02`, or any other code under the startup family)
and the same message it would have seen connecting to PG directly.

pg_doorman does not retry with the parameter removed and does not keep a
per-pool quarantine. The next client connection sends the same
`StartupMessage`; fix the config before routing traffic back to that
pool.

## Observability

The admin SQL console exposes the per-pool effective cascade:

```text
admin> SHOW STARTUP_PARAMETERS;
 user  | database | parameter        | value             | source
-------+----------+------------------+-------------------+-----------
 shop  | checkout | plan_cache_mode  | force_custom_plan | pool
 shop  | reports  | statement_timeout| 10s               | general
```

The Web UI's pool detail page shows the same view in the "Startup
parameters (operator-injected)" section.

Prometheus exports one counter for PG-side rejections:

- `pg_doorman_backend_startup_parameter_errors_total{pool, sqlstate}`
  counts every backend startup PostgreSQL rejected because of an
  operator-supplied parameter. The failing parameter name and
  username are written to the warning log line, not to metric labels.

Alert on a non-zero
`pg_doorman_backend_startup_parameter_errors_total` rate for the same
pool over several minutes. That usually means new backend startups for
the pool are failing on the same operator GUC.

## When not to use this

- The application already sets the parameter on every connection.
  Duplicating the value in `startup_parameters` adds another config path
  without changing behavior.
- Per-transaction tuning (`SET LOCAL`). `startup_parameters` is for
  session defaults; transaction-scoped tuning belongs in the
  application.
- Anything that needs to depend on which query the application is
  running. Startup parameters apply to every transaction on every
  backend for the lifetime of that backend; there is no
  per-statement variant.

## Reference

- [General Settings](../reference/general.md): `startup_parameters`.
- [Pool Settings](../reference/pool.md):
  `pools.<name>.startup_parameters`.
- [auth_query](../authentication/auth-query.md): passthrough vs
  dedicated modes, where the `startup_parameters` column is read.
- [Admin Commands](../observability/admin-commands.md):
  `SHOW STARTUP_PARAMETERS`.
- [Prometheus](../reference/prometheus.md): full metric list.
