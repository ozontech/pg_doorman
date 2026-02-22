# auth_query Feature Design

## Overview

`auth_query` allows pg_doorman to dynamically fetch user credentials from PostgreSQL
instead of requiring all passwords in the static configuration file. This enables
password rotation, centralized credential management, and dynamic user provisioning
without pooler restarts.

## Competitor Analysis

### PgBouncer Problems

| # | Problem | Severity | Issue |
|---|---------|----------|-------|
| 1 | No caching — query on every connection, 2x load on PG | HIGH | [#1302](https://github.com/pgbouncer/pgbouncer/issues/1302) |
| 2 | Connection blocking — client hangs 120s when auth_user fails | HIGH | [#649](https://github.com/pgbouncer/pgbouncer/issues/649) |
| 3 | **Security: failed lookup connects as auth_user** | CRITICAL | [#69](https://github.com/pgbouncer/pgbouncer/issues/69) |
| 4 | `[users]` section skips auth_query entirely | MEDIUM | [#484](https://github.com/pgbouncer/pgbouncer/issues/484) |
| 5 | Chicken-and-egg: auth_user must be in auth_file | MEDIUM | [#967](https://github.com/pgbouncer/pgbouncer/issues/967) |
| 6 | Auth function must exist in every database | MEDIUM | [#1263](https://github.com/pgbouncer/pgbouncer/issues/1263) |
| 7 | SCRAM segfaults under high concurrency (10k clients) | HIGH | [#1452](https://github.com/pgbouncer/pgbouncer/issues/1452) |
| 8 | SCRAM regression with forced user in 1.23 | MEDIUM | [#1343](https://github.com/pgbouncer/pgbouncer/issues/1343) |
| 9 | Pool explosion with dynamic users (227k pools, no GC) | HIGH | [#1085](https://github.com/pgbouncer/pgbouncer/issues/1085) |
| 10 | search_path security in auth functions | MEDIUM | [#1163](https://github.com/pgbouncer/pgbouncer/issues/1163) |
| 11 | VALID UNTIL not checked in default auth_query | HIGH | CVE-2025-2291 |
| 12 | Forced user + auth_query two-phase confusion | MEDIUM | [#1072](https://github.com/pgbouncer/pgbouncer/issues/1072) |

**Key takeaway:** PgBouncer has no auth_query caching at all. Every client connection
triggers a SQL query to PostgreSQL. The proposed fix (cache until auth failure, then
re-query) has been discussed since 2023 but never implemented.

### Odyssey Problems

| # | Problem | Severity | Issue |
|---|---------|----------|-------|
| 1 | **Wrong cache key: `user->name` (param name) vs `user->value` (actual username)** | CRITICAL | [#541](https://github.com/yandex/odyssey/issues/541) |
| 2 | Segfault from uninitialized cache memory | HIGH | [#536](https://github.com/yandex/odyssey/issues/536) |
| 3 | SQL injection via `%u` string interpolation | CRITICAL | [#149](https://github.com/yandex/odyssey/issues/149) |
| 4 | Missing internal routing config requirement | MEDIUM | [#468](https://github.com/yandex/odyssey/issues/468) |
| 5 | Protocol mismatch with MD5 backend auth | MEDIUM | [#105](https://github.com/yandex/odyssey/issues/105) |
| 6 | SCRAM segfault with OpenSSL mismatch | MEDIUM | [#532](https://github.com/yandex/odyssey/issues/532) |
| 7 | Crashes under load (>5 RPS) with auth_query | HIGH | [#352](https://github.com/yandex/odyssey/issues/352) |

**Odyssey cache bug #541 in detail:** Odyssey uses `kiwi_var_t` structs for startup
parameters. The struct has `name` (parameter name, e.g. literal string `"user"`) and
`value` (the actual value, e.g. `"alice"`). The cache used `user->name` as the hashmap
key — meaning ALL users mapped to the same cache entry (the string `"user"`). The first
user's password was cached, and all subsequent users within the 10s TTL received that
wrong password. Fixed by changing to `user->value` in PR #547.

**Odyssey cache bug #536:** `od_auth_cache_value_t` was `malloc`'d without `memset(0)`.
The `passwd` pointer field contained garbage. Later code checked `if (passwd != NULL)`
before freeing — but garbage passed the check, causing segfault via double-free/heap
corruption. Classic C memory safety bug that Rust eliminates by design.

### Odyssey Implementation Details

- Cache: hashmap with **10-second fixed TTL**, per-username key
- Query: parameterized `$1` (after fixing SQL injection)
- Internal client: routes through Odyssey's own routing system
- Auth user: must use `trust` authentication (no password)
- SCRAM: parses SCRAM verifier from pg_shadow, falls back to plain password

## Proposed Architecture for pg_doorman

### Design Principle: "Cache, re-query on auth failure"

When a client's password doesn't match the cached hash, we assume the password
may have changed in PostgreSQL and re-fetch the hash via auth_query. This re-fetch
is rate-limited to at most once per `min_interval` (default 1s) per username to
prevent abuse. After re-fetch, we verify the client's password again — if it still
doesn't match, we reject.

This approach (proposed in PgBouncer #1302 discussion) provides:
- Near-zero load on PG (unlike PgBouncer's query-per-connection)
- Fast password change detection (1 failed attempt → immediate re-fetch → retry)
- Brute-force / thundering herd protection (re-fetch limited to once per `min_interval` per username)

### Configuration

All auth_query settings are grouped in a nested `auth_query` section within the pool:

```yaml
pools:
  mydb:
    server_host: "10.0.0.1"
    server_port: 5432
    pool_mode: "transaction"

    auth_query:
      query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
      user: "pg_doorman_auth"         # user to execute the query
      password: "secret"              # plaintext password for that user
      database: "postgres"            # database for auth queries (default: pool name)
      pool_size: 2                    # connections for executing auth queries (default: 2)
      default_pool_size: 40           # pool_size per dynamic user (default: 40)
      cache_ttl: "1h"                 # max cache age (default: 1h)
      cache_failure_ttl: "30s"        # "user not found" cache TTL (default: 30s)
      min_interval: "1s"              # rate limit re-fetch per user on auth failure (default: 1s)

    users:
      # Static user — auth_query NOT used (explicit password takes priority)
      - username: "app_static"
        password: "md5..."
        pool_size: 40
        server_username: "app"
        server_password: "secret"

      # Dynamic users via auth_query don't need entries here.
      # Each gets their own pool (pool_size from auth_query.default_pool_size).
```

TOML equivalent:

```toml
[pools.mydb.auth_query]
query = "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
user = "pg_doorman_auth"
password = "secret"
database = "postgres"
pool_size = 2
default_pool_size = 40
cache_ttl = "1h"
cache_failure_ttl = "30s"
min_interval = "1s"
```

### Authentication Source Resolution

```
Client connects as username@database
  │
  ├─ username found in static users with password?
  │   ├─ Yes → static authentication (current mechanism)
  │   └─ No → auth_query configured for pool?
  │       ├─ Yes → auth_query authentication
  │       └─ No → reject "unknown user"
```

Static users with explicit `password` ALWAYS take priority. This cleanly resolves
PgBouncer's #484 (static vs dynamic user conflict).

### Cache Design

```
AuthQueryCache (per-pool, DashMap<String, CacheEntry>)

  key: username (actual username, NOT rule/config name — Odyssey #541 lesson)

  value: CacheEntry {
      password_hash: String,        // "md5..." or "SCRAM-SHA-256$..."
      fetched_at: Instant,          // when fetched from PG
      is_negative: bool,            // true = user not found in pg_shadow
  }
```

**Invalidation strategy:**

1. **Positive entry** (password found): lives for `cache_ttl` (default 1h).
   Invalidated immediately on authentication failure.
2. **Negative entry** (user not found): lives for `cache_failure_ttl` (default 30s).
   Protects against DoS via non-existent username enumeration.
3. **Rate limiting on re-fetch**: when auth fails and we want to re-query PG
   (maybe password changed), we won't do it more than once per `min_interval` (1s)
   per username. Protects against brute-force and thundering herd.

### Authentication Flow (detailed)

```
1. Client connects as "alice"@"mydb"
2. Look up static User with username="alice" → not found
3. Check auth_query configured for pool "mydb" → yes
4. Get password hash (cache or fetch):

   4a. Cache HIT (not expired by cache_ttl):
       → Use cached password_hash
   4b. Cache MISS or expired:
       → Fetch from PG (step 7) → cache result

5. Authenticate client against the hash:
   → MD5 prefix "md5": MD5 challenge-response
   → SCRAM prefix "SCRAM-SHA-256$": SCRAM-SHA-256 handshake
   → NULL password: REJECT
   → Success? Done.
   → Failure? Go to step 6

6. Auth failed — maybe password changed in PG? Re-fetch:

   6a. Was the cached entry used (not a fresh fetch)?
       → No (just fetched in step 4b): REJECT — password is wrong
       → Yes: continue to 6b

   6b. Rate limit: last re-fetch for "alice" was < min_interval (1s) ago?
       → Yes: REJECT — won't hammer PG for the same user
       → No: re-fetch from PG (step 7)

7. Execute auth_query:

   7a. Connect to auth_query.database as auth_query.user
       → Connection error? FAIL FAST, clear error message
         (never retry-loop — PgBouncer #649 lesson)

   7b. Execute: SELECT usename, passwd FROM pg_shadow WHERE usename = $1
       ($1 = "alice", parameterized — Odyssey #149 lesson)

   7c. Parse response:
       → 0 rows: cache negative entry, REJECT ("user not found")
         (NEVER fall through to auth_user — PgBouncer #69 lesson)
       → 1 row: cache password hash
       → >1 rows: log warning, take first row
       → SQL error: log error, REJECT (fail fast)

8. Re-authenticate with fresh hash:
   → Success? Done (password was rotated, new hash works)
   → Failure? REJECT — password is wrong
```

### Pool Architecture

Two types of pools serve auth_query:

**1. Auth query executor pool** (`auth_query.pool_size`, default: 2)
- Connects to `auth_query.database` (typically "postgres") as `auth_query.user`
- Used ONLY for executing `SELECT FROM pg_shadow` queries
- Small — auth queries are rare thanks to caching
- Lazy initialization on first auth_query request

This solves PgBouncer's bootstrap problem (#967) — `auth_query.user` and
`auth_query.password` are in the config, no separate auth_file needed.
Simpler than Odyssey's approach (which requires trust in pg_hba.conf).

**2. Per-user data pools** (`auth_query.default_pool_size`, default: 40)
- Each dynamic user gets their OWN connection pool (like static users)
- Connects to the target database as `auth_query.user`
- Pool is created on first successful authentication
- Inactive pools are cleaned up after `idle_timeout` (from general config)

```
Pool "mydb":
  ├─ app_static → own pool (pool_size=40, server_user="app")      ← static
  ├─ alice      → own pool (pool_size=40, server_user="pg_doorman_auth") ← dynamic
  ├─ bob        → own pool (pool_size=40, server_user="pg_doorman_auth") ← dynamic
  └─ [auth_query executor] (pool_size=2, database="postgres")
       └─ SELECT FROM pg_shadow only
```

Dynamic user pools use `auth_query.user`/`auth_query.password` as backend
credentials (server_username/server_password). Inactive dynamic pools are
garbage-collected using the existing `idle_timeout` mechanism — when all
connections in a dynamic user's pool have been idle beyond the timeout,
the pool is removed entirely.

### Security Mitigations

| Threat | Mitigation |
|--------|------------|
| SQL injection | Parameterized query `$1`, no string interpolation |
| auth_user context leak | Never pre-populate; 0 rows = reject |
| search_path attack | Documentation with `SET search_path = pg_catalog` |
| Thundering herd | `min_interval` rate limiting per username |
| Cache key confusion | Key = actual username (not config section name) |
| VALID UNTIL bypass | Recommended query includes `valuntil` check |
| Client blocking | Fail fast with `connect_timeout` |
| Memory safety (segfaults) | Rust ownership model eliminates this class |
| Pool explosion | Per-user pools with idle GC (PgBouncer #1085 lesson) |

### Recommended Auth Function

```sql
CREATE OR REPLACE FUNCTION pg_doorman_lookup(p_username TEXT)
RETURNS TABLE(username NAME, password TEXT) AS $$
BEGIN
  RETURN QUERY
  SELECT usename, passwd::text
  FROM pg_catalog.pg_shadow
  WHERE usename = p_username
    AND (valuntil IS NULL OR valuntil > now());
END;
$$ LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, pg_temp;

-- Usage in config:
-- auth_query:
--   query: "SELECT * FROM pg_doorman_lookup($1)"
```

### Code Changes (scope)

| File | Changes |
|------|---------|
| `src/config/pool.rs` | New `AuthQueryConfig` struct as `Option<AuthQueryConfig>` field on `Pool` |
| `src/auth/mod.rs` | New branch in `authenticate_normal_user()` for auth_query lookup |
| `src/auth/auth_query.rs` (new) | `AuthQueryCache`, `AuthQueryExecutor`, query + cache logic |
| `src/pool/mod.rs` | Dynamic pool creation on first auth, idle pool GC |
| `src/server/server_backend.rs` | `query_one_row()` method to return DataRow |

### Comparison Table

| | pg_doorman (proposed) | PgBouncer | Odyssey |
|---|---|---|---|
| Caching | Cache until auth failure + TTL | None | 10s fixed TTL |
| Rate limiting | Configurable per-username | None | Implicit via TTL |
| Auth user credentials | Nested in pool config | Separate auth_file | trust in pg_hba.conf |
| Auth database | Configurable `database` | `auth_dbname` | `auth_query_db` |
| Dynamic user pools | Per-user with idle GC | Per-user (no GC, explosion) | Per-user (no GC) |
| SQL injection protection | Parameterized `$1` | Parameterized `$1` | Parameterized `$1` |
| Memory safety | Rust (no segfaults) | C (segfaults reported) | C (segfaults reported) |
| SCRAM support | Full | Buggy under concurrency | Full (after fixes) |
