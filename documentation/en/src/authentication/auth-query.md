# auth_query

Look up user credentials from PostgreSQL itself instead of listing every user in the pool config. Useful when users are provisioned dynamically or rotated frequently.

## Two modes

PgDoorman supports two modes; both are configured in the same `auth_query` block. Choose by whether you set `server_user`:

- **Passthrough mode** (no `server_user`): each authenticated user gets its own backend pool, authenticated as that user. Preserves per-user backend identity for `current_user`, row-level security, and audit logs.
- **Dedicated mode** (with `server_user`): all dynamic users share a single backend pool authenticated as `server_user`. Trades per-user identity for higher pool reuse and lower connection count.

PgBouncer-style auth_query is dedicated mode. Odyssey supports both. PgDoorman's passthrough mode is the default.

## Passthrough mode

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    auth_query:
      query: "SELECT passwd FROM pg_shadow WHERE usename = $1"
      user: "postgres"
      password: "md5..."
      database: "postgres"
      cache_ttl: "1h"
      cache_failure_ttl: "30s"
```

The query must return a column named `passwd` or `password` containing the MD5 or SCRAM hash. Extra columns are ignored.

`user` and `password` are the credentials PgDoorman uses to run the lookup query. They must have permission to read the credential column. Either grant access to a custom view (recommended) or use a user in `pg_read_server_files` group.

When a client connects as `alice`:

1. PgDoorman runs the query with `$1 = 'alice'` and gets her hash.
2. Caches the hash in memory for `cache_ttl` seconds.
3. Performs MD5 or SCRAM passthrough authentication (see [Passthrough](passthrough.md)).
4. Opens a backend connection authenticated as `alice` with the same hash.

## Dedicated mode

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    auth_query:
      query: "SELECT passwd FROM pg_shadow WHERE usename = $1"
      user: "auth_lookup"
      password: "md5..."
      database: "postgres"
      server_user: "app"
      server_password: "md5..."
      pool_size: 40
      min_pool_size: 5
      cache_ttl: "1h"
```

Setting `server_user` switches the mode. Now:

1. The client authenticates as `alice` against the hash returned by the query.
2. The backend pool is authenticated as `app` (the `server_user`), and is shared across all dynamic users.
3. `current_user` in PostgreSQL will always be `app`, regardless of which client connected.

Use this when you have many users (thousands) and per-user backend pools would exhaust PostgreSQL's connection slots.

## Recommended PostgreSQL setup

Avoid using a superuser for the lookup. Create a dedicated function with `SECURITY DEFINER`:

```sql
CREATE OR REPLACE FUNCTION pg_doorman_lookup(uname text)
RETURNS TABLE(passwd text)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
  SELECT passwd FROM pg_shadow WHERE usename = uname;
$$;

REVOKE ALL ON FUNCTION pg_doorman_lookup(text) FROM public;
GRANT EXECUTE ON FUNCTION pg_doorman_lookup(text) TO auth_lookup;
```

Then in the pool config:

```yaml
auth_query:
  query: "SELECT passwd FROM pg_doorman_lookup($1)"
  user: "auth_lookup"
  password: "md5..."
```

## Caching

| Parameter | Default | Purpose |
| --- | --- | --- |
| `cache_ttl` | `"1h"` | How long a successful lookup is cached. |
| `cache_failure_ttl` | `"30s"` | How long a failed lookup is cached. Prevents brute-force amplification. |
| `min_interval` | `"1s"` | Minimum interval between repeated lookups for the same user. |

Duration values are quoted strings: `"1h"`, `"30m"`, `"300s"`. A bare integer is interpreted as milliseconds — `cache_ttl: 3600` would cache for 3.6 seconds, not one hour.

Cache is per-pool, in-memory, evicted on `RELOAD`. Restart or `RELOAD` after rotating a user's password.

## Observability

`SHOW AUTH_QUERY` exposes per-database stats:

```
database | cache_entries | cache_hits | cache_misses | cache_refetches | rate_limited | auth_success | auth_failure | executor_queries | executor_errors
```

Prometheus metrics: `pg_doorman_auth_query_cache`, `pg_doorman_auth_query_auth`, `pg_doorman_auth_query_executor`, `pg_doorman_auth_query_dynamic_pools`. See [Admin commands](../observability/admin-commands.md).
