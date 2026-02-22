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

### Design Principle: "Cache forever, re-query on failure"

This approach (proposed in PgBouncer #1302 discussion) provides:
- Near-zero load on PG (unlike PgBouncer's query-per-connection)
- Fast password change detection (1 failed attempt → cache refresh)
- Thundering herd protection (one query per N concurrent failures)

### Configuration (per-pool level)

```yaml
pools:
  mydb:
    server_host: "10.0.0.1"
    server_port: 5432

    # --- auth_query settings ---
    auth_query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
    auth_query_user: "pg_doorman_auth"       # user to execute the query
    auth_query_password: "secret"            # plaintext password for auth_query_user
    auth_query_database: "postgres"          # database to connect to (default: pool name)
    auth_query_cache_ttl: "1h"               # max cache lifetime (default: 1h, 0 = forever)
    auth_query_cache_failure_ttl: "30s"      # TTL for "user not found" entries (default: 30s)
    auth_query_min_interval: "1s"            # min interval between queries per username

    users:
      # Static user — auth_query NOT used (explicit password takes priority)
      - username: "app_static"
        password: "md5..."
        pool_size: 40
        server_username: "app"
        server_password: "secret"

      # Dynamic users via auth_query don't need entries here
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
3. **Rate limiting**: no more than one query per `min_interval` (1s) per username.
   Protects against thundering herd on mass password rotation.

### Authentication Flow (detailed)

```
1. Client connects as "alice"@"mydb"
2. Look up static User with username="alice" → not found
3. Check auth_query configured for pool "mydb" → yes
4. Check cache:

   4a. Cache HIT (TTL not expired):
       → Use cached password_hash for MD5/SCRAM authentication
       → Success? Done.
       → Failure? Go to step 5

   4b. Cache MISS or TTL expired:
       → Go to step 5

5. Execute auth_query (with rate limiting):

   5a. Check min_interval: last query for "alice" was < 1s ago?
       → Yes: use cached result (even if auth failed)
       → No: execute query

   5b. Connect to auth_query_database as auth_query_user
       → Connection error? FAIL FAST, clear error message
         (never retry-loop — PgBouncer #649 lesson)

   5c. Execute: SELECT usename, passwd FROM pg_shadow WHERE usename = $1
       ($1 = "alice", parameterized — Odyssey #149 lesson)

   5d. Parse response:
       → 0 rows: cache negative entry, REJECT ("user not found")
         (NEVER fall through to auth_user — PgBouncer #69 lesson)
       → 1 row: cache password, use for authentication
       → >1 rows: log warning, take first row
       → SQL error: log error, REJECT (fail fast)

6. Authenticate with fetched hash:
   → MD5 prefix "md5": MD5 challenge-response
   → SCRAM prefix "SCRAM-SHA-256$": SCRAM-SHA-256 handshake
   → NULL password: REJECT

7. If auth failed AND cache was stale → refresh from PG (step 5)
   If auth failed AND cache was fresh → REJECT
```

### auth_query_user Connection

Dedicated persistent connection per pool (not from the main pool):
- Lazy initialization on first auth_query request
- Automatic reconnection on failure
- Does not consume pool_size slots
- Plaintext password in config (like `server_password`)

This solves PgBouncer's bootstrap problem (#967) — no separate auth_file needed.
Simpler than Odyssey's approach (which requires trust in pg_hba.conf).

### Dynamic User Pool Management

When auth_query authenticates a new user, a pool is created with defaults:
- `pool_size`: from pool-level `default_pool_size` (default: 20)
- `pool_mode`: inherited from pool
- `server_username`: `auth_query_user` (reused for backend)
- `server_password`: `auth_query_password`

**Pool explosion protection** (PgBouncer #1085):

```yaml
general:
  auth_query_max_users: 1000            # max dynamic users (default: 1000)
  auth_query_user_idle_timeout: "30m"   # remove idle dynamic user pools
```

- Dynamic pools have idle timeout — removed after N minutes of inactivity
- Hard limit on dynamic user count — new users rejected at limit
- Metric: `pg_doorman_auth_query_dynamic_users_total` gauge

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

-- Usage:
-- auth_query: "SELECT * FROM pg_doorman_lookup($1)"
```

### Code Changes (scope)

| File | Changes |
|------|---------|
| `src/config/pool.rs` | New fields: `auth_query`, `auth_query_user`, `auth_query_password`, `auth_query_database`, `auth_query_cache_ttl`, `auth_query_cache_failure_ttl`, `auth_query_min_interval` |
| `src/config/general.rs` | `auth_query_max_users`, `auth_query_user_idle_timeout` |
| `src/auth/mod.rs` | New branch in `authenticate_normal_user()` for auth_query lookup |
| `src/auth/auth_query.rs` (new) | `AuthQueryCache`, `AuthQueryConnection`, query + cache logic |
| `src/pool/mod.rs` | Dynamic pool creation/removal for auth_query users |
| `src/server/server_backend.rs` | `query_one_row()` method to return DataRow |

### Comparison Table

| | pg_doorman (proposed) | PgBouncer | Odyssey |
|---|---|---|---|
| Caching | Cache until auth failure + TTL | None | 10s fixed TTL |
| Rate limiting | Configurable per-username | None | Implicit via TTL |
| Auth user credentials | In config (plaintext) | In auth_file | trust in pg_hba.conf |
| Auth database | Configurable `auth_query_database` | `auth_dbname` | `auth_query_db` |
| Dynamic pool GC | Idle timeout + max limit | None (pool explosion) | None |
| SQL injection protection | Parameterized `$1` | Parameterized `$1` | Parameterized `$1` |
| Memory safety | Rust (no segfaults) | C (segfaults reported) | C (segfaults reported) |
| SCRAM support | Full | Buggy under concurrency | Full (after fixes) |
