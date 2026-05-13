# PostgreSQL startup parameters

Use `startup_parameters` when a pool needs PostgreSQL GUC defaults at
backend startup and you do not want to change `postgresql.conf`,
`ALTER ROLE`, or `ALTER DATABASE`.

- A hot OLTP pool gets stuck on a generic plan after the
  `plan_cache_mode = auto` heuristic flips. Setting
  `force_custom_plan` on the role would affect every workload using
  that role; setting it on one pool keeps the change local.
- An application that does not set its own `statement_timeout` or
  `idle_in_transaction_session_timeout` and cannot be patched fast
  enough. The DBA needs a server-side default that survives the
  application's own session resets.
- A single application that should announce a stable
  `application_name` regardless of what the connecting driver
  negotiates, so `pg_stat_activity` and audit logs stay legible.

## Configuration

Values apply in three layers. The more specific layer wins per key:

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

The column may be `text`, `json`, or `jsonb`; pg_doorman dispatches by
the column type without requiring a cast. The content must be a JSON
object whose values are strings. Other PostgreSQL types (or a custom
domain on top of `jsonb`) log a warning and the per-user overlay is
ignored.

Dedicated `auth_query` mode (`server_user` set) ignores the per-user
column and logs once per (pool, username): one shared backend serves
many users, so a per-user override cannot apply.

## What pg_doorman does with the values

pg_doorman adds the resolved parameter set to the PostgreSQL
`StartupMessage` for each new backend. PostgreSQL records each value as
the session default for that setting (`pg_settings.reset_val` and
`pg_settings.source = 'client'`), so client-side `RESET ALL` and
`DISCARD ALL` return to the configured value. Operators get a stable
session default without editing `postgresql.conf` or running
`ALTER ROLE`.

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
- Reserved keys (`user`, `database`, `replication`, `options`, `role`,
  `session_authorization`, and anything starting with `_pq_.`) are
  refused. pg_doorman manages them itself or PG treats them specially in
  the StartupMessage.
- Values must not contain null bytes.
- Each level (general or per-pool) must fit within the startup-parameter
  budget: `MAX_STARTUP_PACKET_LENGTH` (10 000 bytes) minus 512 bytes
  reserved for pg_doorman-managed keys.

Before each backend spawn pg_doorman checks the resolved parameter set
against the same cap. Layers that fit individually can overflow once
they are merged: general + pool may already exceed the cap, and an
`auth_query` overlay can push a previously fitting cascade over the
limit. Any overflow — overlay-only or baseline-side — is now reported
as a PostgreSQL-style error (`SQLSTATE 53400`) on the client connection
instead of silently shipping a partial or empty StartupMessage. The
warn log line at pool construction records the byte counts; the
`pg_doorman_startup_parameters_dropped_total` counter ticks for every
rejected backend spawn.

## What happens when PG rejects a parameter

If PostgreSQL rejects a configured parameter at backend startup,
pg_doorman returns PostgreSQL's `ErrorResponse` to the client unchanged.
The client sees the same sqlstate (`22023`, `42704`, `42501`, `55P02`,
or any other code under the startup family) and the same message it
would have seen when connecting to PostgreSQL directly.

pg_doorman does not retry with the parameter removed and does not
automatically disable that key for the pool. The next client connection
sends the same `StartupMessage` and gets the same error until the
operator fixes the config.

## Observability

The admin SQL console shows the resolved parameters for each pool:

```text
admin> SHOW STARTUP_PARAMETERS;
 user | database | parameter         | value             | source  | state
------+----------+-------------------+-------------------+---------+--------
 shop | checkout | plan_cache_mode   | force_custom_plan | pool    | applied
 shop | reports  | statement_timeout | 10s               | general | applied
```

The Web UI shows the same rows on the pool detail page in the "Startup
parameters (configured)" section.

Prometheus exports counters for both failure points:

- `pg_doorman_backend_startup_parameter_errors_total{pool, sqlstate}`
  counts every backend startup PostgreSQL rejected because of an
  configured parameter. The failing parameter name and
  username are written to the warning log line, not to metric labels.
- `pg_doorman_startup_parameters_dropped_total{pool, reason}` counts
  parameter sets pg_doorman dropped before sending `StartupMessage`.

Alert when `pg_doorman_backend_startup_parameter_errors_total` keeps
growing for the same pool for several minutes. That usually means new
backend startups for the pool are failing on the same configured GUC.

## When not to use this

- The application already sets the parameter on every connection.
  Duplicating the value in `startup_parameters` adds another config path
  and does not change runtime behavior.
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
