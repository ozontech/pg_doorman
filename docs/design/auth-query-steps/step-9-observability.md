# Step 9: Observability (admin commands + Prometheus)

## Goal

Make auth_query visible in admin console and Prometheus metrics.
Operators need to debug cache behavior, executor health, and dynamic pool count.

## Dependencies

- Step 4 (cache and executor exist)
- Independent of Steps 5-8 (can be done in parallel after Step 4)

## 9.1 Admin commands

### SHOW POOLS — dynamic pool marker

Find where `SHOW POOLS` is implemented (likely `src/admin/mod.rs` or similar).
Add a `source` column or marker:

```
database | user   | ... | source
---------+--------+-----+------------
mydb     | static | ... | config
mydb     | alice  | ... | auth_query
mydb     | bob    | ... | auth_query
```

Implementation: check `is_dynamic_pool(&PoolIdentifier)` when building SHOW POOLS
response. If dynamic → source = "auth_query", else source = "config".

### SHOW CLIENTS — auth source

Add `auth_source` column to SHOW CLIENTS output:

```
database | user  | ... | auth_source
---------+-------+-----+-------------
mydb     | static| ... | config
mydb     | alice | ... | auth_query
```

This requires storing the auth source on the client session. Add a field to
whatever struct represents a connected client:

```rust
pub enum AuthSource {
    Config,      // Static user from config
    AuthQuery,   // Dynamic user via auth_query
}
```

### SHOW AUTH_QUERY_CACHE (new command)

New admin command for debugging auth_query cache state:

```
database | username | password_type | cached_at           | expires_in | is_negative | last_refetch
---------+----------+--------------+--------------------+------------+-------------+-------------
mydb     | alice    | SCRAM         | 2024-01-01 12:00:00 | 3540s      | false       | never
mydb     | bob      | MD5           | 2024-01-01 12:01:00 | 3480s      | false       | 12:01:30
mydb     | nobody   | -             | 2024-01-01 12:02:00 | 25s        | true        | never
```

Implementation: iterate over all `AUTH_QUERY_STATE` entries and their caches.

### SHOW AUTH_QUERY_EXECUTORS (optional, useful for debugging)

```
database | user             | database   | pool_size | active | status
---------+------------------+-----------+-----------+--------+--------
mydb     | pg_doorman_auth  | postgres  | 2         | 0      | ready
otherdb  | pg_doorman_auth  | postgres  | 2         | 1      | ready
```

## 9.2 Prometheus metrics

### File: wherever Prometheus metrics are defined

Add new metrics:

```rust
// Cache metrics
lazy_static! {
    static ref AUTH_QUERY_CACHE_HITS: IntCounterVec = register_int_counter_vec!(
        "pg_doorman_auth_query_cache_hits_total",
        "Number of auth_query cache hits",
        &["database"]
    ).unwrap();

    static ref AUTH_QUERY_CACHE_MISSES: IntCounterVec = register_int_counter_vec!(
        "pg_doorman_auth_query_cache_misses_total",
        "Number of auth_query cache misses (triggered PG query)",
        &["database"]
    ).unwrap();

    static ref AUTH_QUERY_CACHE_REFETCHES: IntCounterVec = register_int_counter_vec!(
        "pg_doorman_auth_query_cache_refetches_total",
        "Number of auth_query re-fetches on auth failure",
        &["database"]
    ).unwrap();

    static ref AUTH_QUERY_CACHE_RATE_LIMITED: IntCounterVec = register_int_counter_vec!(
        "pg_doorman_auth_query_cache_rate_limited_total",
        "Number of auth_query re-fetches blocked by rate limiting",
        &["database"]
    ).unwrap();

    // Executor metrics
    static ref AUTH_QUERY_EXECUTOR_ACTIVE: IntGaugeVec = register_int_gauge_vec!(
        "pg_doorman_auth_query_executor_active_connections",
        "Number of active executor pool connections",
        &["database"]
    ).unwrap();

    static ref AUTH_QUERY_EXECUTOR_ERRORS: IntCounterVec = register_int_counter_vec!(
        "pg_doorman_auth_query_executor_errors_total",
        "Number of auth_query executor errors",
        &["database"]
    ).unwrap();

    // Dynamic pool metrics
    static ref AUTH_QUERY_DYNAMIC_POOLS: IntGaugeVec = register_int_gauge_vec!(
        "pg_doorman_auth_query_dynamic_pools",
        "Number of active dynamic user pools",
        &["database"]
    ).unwrap();

    static ref AUTH_QUERY_DYNAMIC_POOLS_CREATED: IntCounterVec = register_int_counter_vec!(
        "pg_doorman_auth_query_dynamic_pools_created_total",
        "Number of dynamic pools created",
        &["database"]
    ).unwrap();

    static ref AUTH_QUERY_DYNAMIC_POOLS_DESTROYED: IntCounterVec = register_int_counter_vec!(
        "pg_doorman_auth_query_dynamic_pools_destroyed_total",
        "Number of dynamic pools destroyed (GC, RELOAD, etc.)",
        &["database"]
    ).unwrap();
}
```

### Instrumentation points

Add metric increments in:
- `AuthQueryCache::get_or_fetch()` → cache_hits / cache_misses
- `AuthQueryCache::refetch_on_failure()` → cache_refetches / cache_rate_limited
- `AuthQueryExecutor::fetch_password()` → executor_errors (on Err)
- `create_dynamic_pool()` → dynamic_pools gauge + created counter
- `gc_idle_dynamic_pools()` → dynamic_pools gauge + destroyed counter
- RELOAD cleanup → destroyed counter

## 9.3 BDD tests

Metrics and admin commands are typically tested via:
- Connecting to admin console and running SHOW commands
- Curling the Prometheus endpoint and checking for metric names

```gherkin
@auth-query @admin
Scenario: SHOW POOLS shows dynamic pool marker
  Given "alice" authenticated via auth_query
  When admin runs SHOW POOLS
  Then "alice" pool shows source "auth_query"

@auth-query @admin
Scenario: SHOW AUTH_QUERY_CACHE shows cached entries
  Given "alice" authenticated via auth_query (cache populated)
  When admin runs SHOW AUTH_QUERY_CACHE
  Then output shows "alice" with password_type and cached_at

@auth-query @prometheus
Scenario: Prometheus metrics for auth_query
  Given auth_query is configured and "alice" connected
  When scraping /metrics endpoint
  Then pg_doorman_auth_query_cache_hits_total is present
  And pg_doorman_auth_query_dynamic_pools is present
```

## Checklist

- [ ] SHOW POOLS: `source` column (config / auth_query)
- [ ] SHOW CLIENTS: `auth_source` column
- [ ] SHOW AUTH_QUERY_CACHE: new admin command
- [ ] AuthSource enum on client session
- [ ] Prometheus: cache hit/miss/refetch/rate_limited counters
- [ ] Prometheus: executor active/errors
- [ ] Prometheus: dynamic pools gauge + created/destroyed counters
- [ ] Instrument cache, executor, pool creation/destruction
- [ ] BDD tests (3)
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
