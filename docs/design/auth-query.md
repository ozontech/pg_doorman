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

**Example 1: Dedicated server user** (all dynamic users → one PG identity)

```yaml
pools:
  mydb:
    server_host: "10.0.0.1"
    server_port: 5432
    pool_mode: "transaction"

    auth_query:
      query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
      user: "pg_doorman_auth"         # executor: runs SELECT FROM pg_shadow
      password: "secret_exec"         # executor password (plaintext)
      database: "postgres"            # executor database (default: pool name)
      pool_size: 2                    # executor connections (default: 2, opened at startup)
      server_user: "app_service"      # backend user for data connections
      server_password: "secret_app"   # backend password (plaintext)
      default_pool_size: 40           # data pool size (default: 40)
      cache_ttl: "1h"                 # max cache age (default: 1h)
      cache_failure_ttl: "30s"        # "user not found" cache TTL (default: 30s)
      min_interval: "1s"              # rate limit re-fetch on auth failure (default: 1s)
```

In this mode all dynamic users share ONE data pool (same backend identity).

**Example 2: Passthrough** (each dynamic user → their own PG identity)

```yaml
pools:
  mydb:
    auth_query:
      query: "SELECT usename, passwd FROM pg_shadow WHERE usename = $1"
      user: "pg_doorman_auth"
      password: "secret_exec"
      database: "postgres"
      pool_size: 2
      # server_user/server_password NOT set → passthrough mode:
      #   MD5 backend:  pass-the-hash (hash from pg_shadow used directly)
      #   SCRAM backend: SCRAM passthrough (ClientKey extracted from client's proof)
      default_pool_size: 40
```

In this mode each dynamic user gets their OWN data pool.

**Static users** always work as before — auth_query is not used for them:

```yaml
    users:
      - username: "app_static"
        password: "md5..."
        pool_size: 40
        server_username: "app"
        server_password: "secret"
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

   7a. Get connection from auth_query executor pool (always open since startup)
       → Executor pool unavailable (PG restart)? FAIL FAST, clear error
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

#### Executor Pool (`auth_query.pool_size`, default: 2)

- Connects to `auth_query.database` (typically "postgres") as `auth_query.user`
- Used ONLY for executing `SELECT FROM pg_shadow` queries
- **Opened eagerly at startup** and kept alive permanently
- Automatic reconnection with backoff on failure

These connections are **reserved** — they must be established before the data pools
start accepting clients. This prevents a critical deadlock scenario:

> If all PG `max_connections` slots are occupied by data pool connections,
> the executor cannot connect to PG, and no new users can authenticate.
> Since data connections won't be freed while actively used, new auth becomes
> permanently impossible.

By opening executor connections at startup, they are guaranteed to exist regardless
of data pool pressure. DBA must account for them: effective available connections
for data = `max_connections` - `auth_query.pool_size` per pool.

If executor connections are lost (PG restart, network issue), pg_doorman reconnects
with exponential backoff. During reconnection, auth_query falls back to the existing
cache. If cache has no entry for a user, authentication fails with a clear error
message (not a hang — PgBouncer #649 lesson).

This solves PgBouncer's bootstrap problem (#967) — `auth_query.user` and
`auth_query.password` are in the config, no separate auth_file needed.
Simpler than Odyssey's approach (which requires trust in pg_hba.conf).

#### Data Pools — Two Modes

The mode is determined by whether `server_user`/`server_password` are set:

**Mode 1: Dedicated server user** (`server_user` IS set)

All dynamic users share ONE data pool. Backend identity is `server_user`.

```
Pool "mydb":
  ├─ app_static  → own pool (pool_size=40, backend="app")             ← static
  ├─ [auth_query data pool] (pool_size=40, backend="app_service")     ← shared
  │    ├─ alice (client auth via cache)
  │    ├─ bob   (client auth via cache)
  │    └─ ...all dynamic users
  └─ [auth_query executor] (pool_size=2, database="postgres")
```

Simple, works with any backend auth method (MD5, SCRAM, trust).
All dynamic users' queries appear as `server_user` in PG logs/pg_stat_activity.
Best for: microservices where per-user backend identity is not needed.

**Mode 2: Passthrough** (`server_user` NOT set)

Each dynamic user gets their OWN data pool. Backend identity = the user themselves.

```
Pool "mydb":
  ├─ app_static → own pool (pool_size=40, backend="app")           ← static
  ├─ alice      → own pool (pool_size=40, backend="alice")         ← dynamic
  ├─ bob        → own pool (pool_size=40, backend="bob")           ← dynamic
  └─ [auth_query executor] (pool_size=2, database="postgres")
```

How pg_doorman authenticates to PG backend per dynamic user:

| Client auth | Backend auth | Mechanism |
|-------------|-------------|-----------|
| SCRAM | SCRAM | **SCRAM passthrough**: extract `ClientKey` from client's SCRAM proof, reuse for backend |
| MD5 | MD5 | **Pass-the-hash**: use md5 hash from pg_shadow for backend MD5 second pass |
| SCRAM | MD5 | **Pass-the-hash**: compute md5 from... N/A, we only have SCRAM verifier → **FAIL** |
| MD5 | SCRAM | No `ClientKey` available → **FAIL** |
| Any | trust | No auth needed → works |

Incompatible client/backend auth combinations fail with a clear error at connection time.

Best for: setups requiring per-user PG identity (audit, RLS, pg_stat_activity).
Inactive dynamic user pools are garbage-collected via `idle_timeout`.

#### SCRAM Passthrough — How It Works

During client SCRAM authentication, pg_doorman acts as SCRAM server and receives
`ClientProof` from the client. The math:

```
ClientProof     = ClientKey XOR ClientSignature
ClientSignature = HMAC(StoredKey, AuthMessage)
```

pg_doorman knows `StoredKey` (from pg_shadow cache) and `AuthMessage` (from the
handshake), so it can compute `ClientSignature` and extract:

```
ClientKey = ClientProof XOR ClientSignature
```

This `ClientKey` is stored in the client session. When a backend SCRAM challenge
arrives, pg_doorman uses the stored `ClientKey` to compute a fresh `ClientProof`
for the backend's challenge (different `AuthMessage`, different `ClientSignature`,
but same `ClientKey`).

**Requirements:**
- Client MUST authenticate with SCRAM (not MD5)
- Backend MUST request SCRAM
- The SCRAM verifier in pg_doorman's cache must match what PG has (naturally true
  since both come from pg_shadow via auth_query)

This is the same technique PgBouncer uses. Odyssey does NOT support it.

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
| Pool explosion | Shared pool with `server_user` OR per-user with idle GC (PgBouncer #1085) |

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
| `src/config/pool.rs` | New `AuthQueryConfig` struct with all fields as `Option<AuthQueryConfig>` on `Pool` |
| `src/auth/mod.rs` | New branch in `authenticate_normal_user()` for auth_query lookup |
| `src/auth/auth_query.rs` (new) | `AuthQueryCache`, `AuthQueryExecutor`, query + cache logic |
| `src/auth/scram.rs` | Extract `ClientKey` from `ClientProof` during SCRAM handshake |
| `src/pool/mod.rs` | Dynamic pool creation on first auth, idle pool GC, shared pool mode |
| `src/server/server_backend.rs` | `query_one_row()` method; MD5 pass-the-hash; SCRAM passthrough with stored `ClientKey` |

### BDD Test Scenarios

```gherkin
# --- Client auth via auth_query ---

Scenario: Auth query with MD5 — valid password
  Given auth_query is configured for pool "mydb"
  And user "alice" exists in pg_shadow with MD5 password
  When client connects as "alice" with correct password
  Then authentication succeeds

Scenario: Auth query with SCRAM — valid password
  Given auth_query is configured for pool "mydb"
  And user "alice" exists in pg_shadow with SCRAM password
  When client connects as "alice" with correct password using SCRAM
  Then authentication succeeds

Scenario: Auth query — wrong password, then password rotation
  Given auth_query is configured for pool "mydb"
  And user "alice" password hash is cached
  When "alice" password is changed in PostgreSQL
  And client connects as "alice" with new password
  Then pg_doorman re-fetches hash from PG
  And authentication succeeds

Scenario: Auth query — wrong password, no rotation
  Given auth_query is configured for pool "mydb"
  When client connects as "alice" with wrong password
  Then authentication fails with "password authentication failed"

Scenario: Auth query — user not found
  Given auth_query is configured for pool "mydb"
  When client connects as "nonexistent" with any password
  Then authentication fails with "user not found"

Scenario: Auth query — static user takes priority
  Given auth_query is configured for pool "mydb"
  And static user "app" is configured with password
  When client connects as "app"
  Then static authentication is used (not auth_query)

Scenario: Auth query — rate limiting on re-fetch
  Given auth_query is configured with min_interval "1s"
  And user "alice" password hash is cached
  When client connects as "alice" with wrong password
  And another client connects as "alice" with wrong password within 1s
  Then only one auth_query is executed against PG

Scenario: Auth query — negative cache TTL
  Given auth_query is configured with cache_failure_ttl "30s"
  When client connects as "nonexistent"
  Then "nonexistent" is cached as negative entry
  And next attempt within 30s does NOT query PG

# --- Backend auth: dedicated server user ---

Scenario: Dedicated server user — all dynamic users share pool
  Given auth_query is configured with server_user "app_service"
  When "alice" and "bob" authenticate via auth_query
  Then both use backend connections as "app_service"
  And pg_stat_activity shows "app_service" for both

# --- Backend auth: MD5 pass-the-hash ---

Scenario: MD5 passthrough — dynamic user as themselves
  Given auth_query is configured WITHOUT server_user
  And PG backend uses MD5 authentication
  When "alice" authenticates via auth_query with MD5
  Then backend connection authenticates as "alice" using hash from pg_shadow
  And pg_stat_activity shows "alice"

# --- Backend auth: SCRAM passthrough ---

Scenario: SCRAM passthrough — dynamic user as themselves
  Given auth_query is configured WITHOUT server_user
  And PG backend uses SCRAM-SHA-256 authentication
  When "alice" authenticates via auth_query with SCRAM
  Then pg_doorman extracts ClientKey from client's SCRAM proof
  And backend connection authenticates as "alice" using SCRAM passthrough
  And pg_stat_activity shows "alice"

Scenario: SCRAM passthrough — incompatible auth methods
  Given auth_query is configured WITHOUT server_user
  And PG backend uses SCRAM-SHA-256 authentication
  When "alice" authenticates via auth_query with MD5 (no ClientKey)
  Then backend connection fails with clear error about incompatible auth

# --- Executor pool ---

Scenario: Executor connections reserved at startup
  Given auth_query is configured with pool_size 2
  When pg_doorman starts
  Then 2 connections to auth_query.database are established immediately
  And they persist even when data pools are full

Scenario: Executor connection loss — fallback to cache
  Given auth_query executor connection is lost
  And user "alice" hash is in cache
  When client connects as "alice" with correct password
  Then authentication succeeds using cached hash
  And pg_doorman reconnects executor in background
```

### Comparison Table

| | pg_doorman (proposed) | PgBouncer | Odyssey |
|---|---|---|---|
| Caching | Cache until auth failure + TTL | None | 10s fixed TTL |
| Rate limiting | Configurable per-username | None | Implicit via TTL |
| Auth user credentials | Nested in pool config | Separate auth_file | trust in pg_hba.conf |
| Auth database | Configurable `database` | `auth_dbname` | `auth_query_db` |
| Backend auth: dedicated user | Yes (`server_user`) | No (uses client identity) | Yes (`storage_user`) |
| Backend auth: MD5 pass-the-hash | Yes | Yes | No |
| Backend auth: SCRAM passthrough | Yes | Yes | No |
| Dynamic user pools | Per-user with idle GC / shared | Per-user (no GC, explosion) | Per-user (no GC) |
| SQL injection protection | Parameterized `$1` | Parameterized `$1` | Parameterized `$1` |
| Memory safety | Rust (no segfaults) | C (segfaults reported) | C (segfaults reported) |
| SCRAM support | Full | Buggy under concurrency | Full (after fixes) |
