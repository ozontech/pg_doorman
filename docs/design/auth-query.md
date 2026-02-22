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
  │   ├─ Yes → static authentication (current mechanism, no changes)
  │   └─ No → auth_query configured for pool?
  │       ├─ Yes → auth_query authentication (see flow below)
  │       └─ No → reject "unknown user"
```

Static users with explicit `password` ALWAYS take priority. This cleanly resolves
PgBouncer's #484 (static vs dynamic user conflict).

### HBA Integration

Current code determines auth method from the password prefix (`md5...` or `SCRAM-SHA-256$...`)
BEFORE running auth. For auth_query users, we don't know the password type until we
fetch from PG. This requires a **two-phase HBA check**:

**Phase 1: HBA pre-check** (BEFORE auth_query, to avoid unnecessary PG queries)

```
Evaluate hba_md5 and hba_scram for this client IP + username + database:
  → Both Deny/NotMatched? → REJECT immediately (don't waste an auth_query call)
  → At least one Trust?   → Set trust_mode = true, continue
  → At least one Allow?   → Continue normally
```

This saves an auth_query round-trip for connections that HBA would reject anyway.

**Phase 2: HBA post-check** (AFTER auth_query returns the password type)

```
Password hash from auth_query:
  → Starts with "md5"            → check hba_md5  → Deny? → REJECT
  → Starts with "SCRAM-SHA-256$" → check hba_scram → Deny? → REJECT
```

This ensures the specific auth method is allowed by HBA.

**Trust mode for dynamic users:**

When HBA says `trust`, pg_doorman still executes auth_query to verify that the user
exists in PostgreSQL (and to cache the password type for future use). But the password
challenge-response is skipped — the client connects without providing a password.
This matches the behavior for static users where `trust` skips the password check
but the user must still be configured.

**pg_hba.conf on PostgreSQL side (documentation requirements):**

1. **Executor connections**: PG must allow `auth_query.user` from pg_doorman's IP
   to `auth_query.database`. Failure → startup error with clear message.
2. **Passthrough mode**: PG must allow dynamic users from pg_doorman's IP to the
   target database. Mismatched rules → clear error at connection time.
3. **Dedicated server_user mode**: PG must allow `server_user` from pg_doorman's IP.
   Only one rule needed regardless of how many dynamic users connect.

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

4. HBA pre-check:
   → hba_md5=Deny AND hba_scram=Deny? → REJECT (no auth_query needed)
   → hba_md5=Trust OR hba_scram=Trust? → trust_mode = true
   → Otherwise: trust_mode = false

5. Get password hash (cache or fetch):
   5a. Cache HIT (not expired by cache_ttl):
       → Use cached password_hash
   5b. Cache MISS or expired:
       → Fetch from PG (step 8) → cache result

6. HBA post-check (now we know the password type):
   → md5 hash + hba_md5=Deny? → REJECT "auth method not allowed by HBA"
   → SCRAM verifier + hba_scram=Deny? → REJECT "auth method not allowed by HBA"

7. Authenticate client:
   → trust_mode? → skip password challenge (user verified to exist in step 5)
   → MD5 prefix "md5": MD5 challenge-response
   → SCRAM prefix "SCRAM-SHA-256$": SCRAM-SHA-256 handshake
   → NULL password: REJECT
   → Success? Create/reuse dynamic user pool → Done.
   → Failure? Go to step 7a

   7a. Auth failed — maybe password changed in PG? Re-fetch:
       → Was the cached entry used (not a fresh fetch)?
         → No (just fetched in step 5b): REJECT — password is wrong
         → Yes: continue to 7b
       → 7b. Rate limit: last re-fetch for "alice" was < min_interval (1s) ago?
         → Yes: REJECT — won't hammer PG for the same user
         → No: re-fetch from PG (step 8), then retry step 7

8. Execute auth_query:
   8a. Get connection from executor pool (always open since startup)
       → Executor unavailable (PG restart)? FAIL FAST, clear error
         (never retry-loop — PgBouncer #649 lesson)

   8b. Execute: SELECT usename, passwd FROM pg_shadow WHERE usename = $1
       ($1 = "alice", parameterized — Odyssey #149 lesson)

   8c. Parse response:
       → 0 rows: cache negative entry, REJECT ("user not found")
         (NEVER fall through to auth_user — PgBouncer #69 lesson)
       → 1 row: cache password hash, return to caller
       → >1 rows: log warning, take first row
       → SQL error: log error, REJECT (fail fast)
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

### Server Parameters for Dynamic Users

After successful authentication, pg_doorman sends `ServerParameters` to the client
(server_version, client_encoding, DateStyle, etc.). These are fetched from the data
pool via `pool.get_server_parameters()`.

**Problem:** For a new dynamic user, the data pool doesn't exist yet at auth time.

**Solution:** Server parameters depend on the PG server, not the user. They are
identical for all connections to the same server. Two approaches by mode:

- **`server_user` mode (shared pool):** The shared pool is created eagerly (like
  the executor pool). Server params are fetched once and cached — all dynamic users
  get the same params. No issue.

- **Passthrough mode (per-user pools):** The per-user pool is created AFTER
  successful auth. Server params can be fetched from the **executor pool connections**
  (same PG server, same params) or from any existing data pool for this database.
  Cache them at pool-level (per database, not per user).

Implementation: add a pool-level `Arc<Mutex<ServerParameters>>` on the auth_query
config that is shared across all dynamic users. Populated on first use from any
available connection (executor or data).

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

# --- HBA integration ---

Scenario: HBA denies both MD5 and SCRAM — no auth_query executed
  Given auth_query is configured for pool "mydb"
  And HBA denies both md5 and scram for client IP
  When client connects as "alice"
  Then connection is rejected immediately
  And no auth_query is executed against PG

Scenario: HBA trust — user exists
  Given auth_query is configured for pool "mydb"
  And HBA trusts connections from client IP
  And user "alice" exists in pg_shadow
  When client connects as "alice" without password
  Then auth_query verifies user exists
  And authentication succeeds without password challenge

Scenario: HBA trust — user does not exist
  Given auth_query is configured for pool "mydb"
  And HBA trusts connections from client IP
  When client connects as "nonexistent" without password
  Then auth_query finds no rows
  And authentication fails with "user not found"

Scenario: HBA allows MD5 but denies SCRAM — user has SCRAM password
  Given auth_query is configured for pool "mydb"
  And HBA allows md5 but denies scram for client IP
  And user "alice" has SCRAM password in pg_shadow
  When client connects as "alice"
  Then authentication fails with "auth method not allowed by HBA"

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

## Open Questions / Known Issues

The following problems have been identified during design review and require
resolution before implementation.

### Problem 1: SCRAM re-auth after password rotation within the same connection

The current flow says: on auth failure with a cached hash, re-fetch from PG and
retry authentication. For MD5 this works — the salt was sent at the start, and
a new `md5(hash + salt)` can be computed.

For SCRAM, the salt is embedded in the SCRAM verifier from pg_shadow and is sent
to the client as part of `ServerFirst`. If the password changes, the verifier
(and salt) change too. But `ServerFirst` was already sent with the OLD salt.
The client computed its proof using the old salt. Re-fetching the new verifier
doesn't help — the client's proof is bound to the old salt.

**Impact:** SCRAM password rotation detection within a single auth attempt is
impossible. The client must reconnect to get a new salt.

**Decision: Option A.** On SCRAM auth failure with stale cache, invalidate
cache entry and reject. The client reconnects, gets the new salt, succeeds.
One extra round-trip on password rotation — acceptable and documented behavior.
Option B (restart handshake) violates SCRAM protocol state machine.

### Problem 2: Request coalescing for concurrent cache misses

When 100 clients connect simultaneously as "alice" and the cache is empty (or
expired), all 100 trigger auth_query in parallel. With `pool_size: 2`, the
executor serializes queries but doesn't know they're for the same username —
two connections could serve two identical "alice" queries while "bob" waits.

**Impact:** Thundering herd on cache miss. Wasted executor bandwidth.

**Decision: Per-username lock with double-checked cache.**

```
1. Check cache → miss
2. Acquire async lock for username (DashMap<String, tokio::sync::Mutex<()>>)
3. Re-check cache → hit? → return, release lock
4. Execute auth_query
5. Populate cache
6. Release lock
```

The first request for "alice" acquires the lock, executes auth_query, populates
the cache. The other 99 "alice" requests wait on the lock; when released, they
find a cache hit and return immediately without touching the executor pool.
Meanwhile, "bob" requests use a different lock and can proceed in parallel.

This is the simplest correct approach — standard double-checked locking pattern
adapted for async with per-key granularity.

### Problem 3: `get_pool()` returns None for new dynamic users

In `src/auth/mod.rs:192`, `get_pool(pool_name, username)` returns `None` for
users not in static config. The current code rejects these immediately. With
auth_query, we need a path to:
1. Recognize that the pool has `auth_query` configured (requires pool-level
   config access, not user-level)
2. Run auth_query flow
3. Create a dynamic pool on success

**Decision: Add `get_pool_config(db: &str) -> Option<&PoolConfig>`** that returns
the pool-level config regardless of user. Auth flow: try `get_pool(db, user)`
first; if None, try `get_pool_config(db)` → check `auth_query` → run auth_query
flow. Clean, linear, doesn't pollute pool map with sentinels.

### Problem 4: Config reload behavior for dynamic pools

When admin runs `RELOAD`, static pools are recreated from the new config.
What happens to dynamic pools?

**Scenarios:**
- auth_query section removed → all dynamic pools must be destroyed
- auth_query section changed (e.g., `default_pool_size` 40→20) → existing
  dynamic pools: keep old size or resize?
- `server_user` added/removed → mode change (shared↔passthrough), all dynamic
  pools become invalid
- Static user added matching a dynamic user → static takes priority, dynamic
  pool should be destroyed

**Decision: On any auth_query config change, destroy ALL dynamic pools.**
Simple and predictable. Existing connections get gracefully closed. The auth_query
cache is also cleared — fresh start. No change to auth_query config = dynamic
pools survive RELOAD untouched.

### Problem 5: Passthrough mode pool scaling

In passthrough mode, each dynamic user gets `default_pool_size` (40) connections.
With 100 users: 100 × 40 = 4000 backend connections. Most PG servers have
`max_connections` = 100-500.

**Decision: Document clearly, defer complexity.** Passthrough mode is for
environments where per-user PG identity matters (audit, RLS) and the user count
is bounded. DBAs choosing this mode understand their `max_connections` and can
tune `default_pool_size` accordingly. Idle pool GC via `idle_timeout` naturally
reclaims unused pools. A global connection limit (`max_dynamic_pool_connections`)
may be added in a future version if real-world demand emerges.

### Problem 6: Admin commands and Prometheus metrics

The design doesn't specify how dynamic pools appear in admin console and metrics.

**Decision: Plan the following additions** (details finalized during implementation):

Admin console:
- `SHOW POOLS`: include dynamic pools with `source=auth_query` marker
- `SHOW CLIENTS`: show whether auth was static or auth_query
- New command: `SHOW AUTH_QUERY_CACHE` — cache entries with TTL, for debugging

Prometheus metrics:
- `pg_doorman_auth_query_cache_hits_total`
- `pg_doorman_auth_query_cache_misses_total`
- `pg_doorman_auth_query_executor_pool_active`
- `pg_doorman_auth_query_dynamic_pools_count`

### Problem 7: Executor pool architecture

The executor pool connects to a DIFFERENT database (`auth_query.database`,
typically "postgres") than the data pools. It uses different credentials
(`auth_query.user`/`auth_query.password`). It's internal-only, never serves
client traffic.

**Decision: Use `deadpool-postgres` directly.** The crate is already a dependency.
Wrap it in an `AuthQueryExecutor` struct with a single method:
`fetch_password(username: &str) -> Result<Option<CacheEntry>>`. This avoids
polluting `ConnectionPool` with internal-pool special cases and keeps the
executor lightweight and purpose-built.
