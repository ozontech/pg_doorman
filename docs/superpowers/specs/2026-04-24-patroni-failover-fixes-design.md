# Patroni Failover Discovery: Fixes

Post-review fixes for the `feature/patroni-failover-discovery` PR.
All changes go into the current branch as separate commits per logical block.

## Block 1: Bugs

### 1.1 FallbackTarget.lifetime_ms ignored

`create_fallback_connection()` returns a `Server` that enters the pool via
`new_object_inner()`. That function calls `Metrics::new(self.server_pool.lifetime_ms(), ...)`
which uses the normal pool lifetime (hours). The `target.lifetime_ms` field is never read.

Fix: add `override_lifetime_ms: Option<u64>` to `Server`. Set it in
`create_fallback_connection` from `target.lifetime_ms`. In `new_object_inner`,
use `obj.override_lifetime_ms.unwrap_or(self.server_pool.lifetime_ms())` as the
base lifetime for `Metrics::new`.

### 1.2 Race in try_connect_candidates

`best` stores the first successful candidate. If leader responds before replica,
replica cannot replace it because of `if best.is_none()`.

Fix: store `best: Option<(String, u16, Role)>`. Replace best when the new
candidate has a strictly better (lower) `role_priority` than the current best.

### 1.3 FAILOVER_HOST_BLACKLISTED gauge never resets

The gauge is set to 1.0 on `blacklist()` but only cleared by `clear()` (SIGHUP).
Natural expiry after 30s leaves gauge at 1.0 forever.

Fix: `is_blacklisted()` returns a `BlacklistCheck` enum:
- `NotBlacklisted` (was never set or manually cleared)
- `Active` (blacklist still in effect)
- `JustExpired` (was blacklisted, now expired — caller should act)

On `JustExpired`: clear `blacklisted_until`, clear `whitelisted_host`, reset
gauge to 0.0, set `blacklist_logged` flag to false.  The caller (`ServerPool::create`)
calls `bump_epoch()` to drain stale fallback connections.

### 1.4 nofailover tag not filtered

`select_candidates()` filters `noloadbalance` and `archive` but not `nofailover`.
A member with `nofailover=true` will never be promoted — sending traffic there
guarantees read-only errors for writes.

Fix: add `&& !m.tags.nofailover` to the filter predicate.

## Block 2: Architecture

### 2.1 Inflight coalescing race condition

After `shared.await`, every waiter clears `*guard = None`. Between the await and
the lock acquisition, another thread may have created a new inflight future. The
current code overwrites it.

Fix: remove the "clear inflight after completion" block entirely. Instead, store
`inflight: Option<(Instant, SharedClusterFuture)>`. When entering
`fetch_cluster_coalesced`, if the stored future was created more than 1 second
ago, treat it as stale and create a new one. This naturally handles both
coalescing (multiple callers within 1s share the same request) and refresh
(after 1s, a new /cluster request is made).

### 2.2 Leaked tokio tasks

`try_connect_candidates` uses `tokio::spawn` for TCP connects. Spawned tasks
are not cancelled when the function returns early (sync_standby found).

Fix: replace `tokio::spawn` with `Box::pin(async move { ... })` and
`futures::future::select_all`, matching the pattern already used in
`PatroniClient::fetch_cluster`. Dropping the `rest` vec cancels remaining
futures.

### 2.3 Whitelist invalidation

Whitelist is never cleared automatically. If the whitelisted host dies, all
subsequent `get_fallback_target()` calls return the dead host.

Fix: two invalidation paths:
1. On blacklist expiry (in `is_blacklisted()` returning `JustExpired`) — clear
   whitelist. Doorman returned to primary, whitelist is stale.
2. On `create_fallback_connection` failure — call
   `failover.clear_whitelist()` so the next call re-runs discovery.

### 2.4 Dead code clear_failover

`ServerPool::clear_failover()` is declared but never called. On SIGHUP, pools
are recreated entirely.

Fix: remove `clear_failover()` from `ServerPool`. Keep `FailoverState::clear()`
as it is used internally by the expiry logic.

### 2.5 reqwest::Client reuse

`PatroniClient::new()` is called on every `fetch_cluster_coalesced`, creating a
new connection pool and TLS context each time.

Fix: store `PatroniClient` in `FailoverState`. Create once in
`FailoverState::new()`. The `expect()` in `PatroniClient::new()` is acceptable
at startup time.

## Block 3: Production reliability

### 3.1 Mutex poisoning

Six `.lock().unwrap()` calls on `std::sync::Mutex`. If any holder panics, all
subsequent locks panic.

Fix: replace `std::sync::Mutex` with `parking_lot::Mutex` for `blacklisted_until`
and `whitelisted_host`. parking_lot::Mutex does not poison. Consistent with the
rest of the project.

### 3.2 Unmarked fallback connections

Fallback connections to a remote host and normal connections to primary share the
same pool. When primary recovers, clients may alternate between servers, breaking
read-after-write consistency.

Fix: when `is_blacklisted()` returns `JustExpired`, `ServerPool::create()` calls
`self.bump_epoch()`. This marks all existing connections (including fallback) as
outdated. `recycle()` closes them on next check. New connections go to primary.

### 3.3 Log flooding

Every `create()` during blacklist logs `info!("failover: primary blacklisted...")`.
With pool_size=100 this produces 100 identical lines.

Fix: add `blacklist_logged: AtomicBool` to `FailoverState`. First message at
`info!`, subsequent at `debug!`. Reset flag on blacklist clear/expiry.

### 3.4 Config validation

`Pool::validate()` does not check failover parameters.

Fix: add validation:
- `patroni_discovery_urls: Some(urls)` where `urls.is_empty()` -> BadConfig
- URLs not starting with `http://` or `https://` -> BadConfig
- `failover_blacklist_duration` is Some(0) -> BadConfig

## Block 4: Metrics

### 4.1 New metrics

- `pg_doorman_failover_fallback_host` — GaugeVec with labels `[pool, host, port]`.
  Set to 1.0 when whitelist is set. Remove label values when whitelist cleared.
- `pg_doorman_failover_whitelist_hits_total` — IntCounterVec with label `[pool]`.
  Incremented on whitelist cache hit in `get_fallback_target()`.

### 4.2 Fix inflated discovery counter

`FAILOVER_DISCOVERY_TOTAL` is incremented for every caller of
`fetch_cluster_coalesced`, including coalesced joiners.

Fix: increment only when creating a new inflight future (in the else branch),
not when joining an existing one.

## Block 5: Tests

### Unit tests (src/pool/failover.rs)

1. `select_candidates_filters_nofailover` — member with nofailover=true excluded
2. `select_candidates_all_replicas` — no sync_standby, replicas selected
3. `select_candidates_only_leader` — single leader candidate returned
4. `blacklist_expiry_clears_whitelist` — after expiry, whitelist is empty

### BDD tests (tests/bdd/features/patroni-failover-discovery.feature)

5. Multiple Patroni URLs, first dead — discovery succeeds via second URL
6. Return to primary — blacklist expires, doorman reconnects to primary
7. All TCP candidates unreachable — client gets error
8. Dynamic member update — mock Patroni response changes mid-test

## Block 6: Documentation

In both EN and RU docs:

1. "Active transactions" section — on PG crash, in-flight transactions get a
   connection error; retry is the client's responsibility
2. "Configuration" — all cluster nodes must accept the same credentials
3. Fallback connections inherit TLS config from primary (unix socket = no TLS)
4. Recommend IP addresses over hostnames for discovery URLs (DNS failure = discovery failure)
5. `standby_leader` role is treated as "other" (lowest priority)
