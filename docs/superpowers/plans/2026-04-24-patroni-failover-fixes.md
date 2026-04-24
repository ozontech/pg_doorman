# Patroni Failover Discovery: Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all bugs, architectural issues, missing tests, metrics, and documentation gaps found in the Patroni failover discovery PR review.

**Architecture:** Six blocks of changes, each block = one commit. Block 1 fixes outright bugs (lifetime not applied, race in candidate selection, gauge staleness, missing tag filter). Block 2 fixes architectural issues (inflight race, leaked tasks, whitelist invalidation, dead code, client reuse). Block 3 hardens production reliability (parking_lot, epoch bump on recovery, log flooding, config validation). Block 4 adds missing metrics. Block 5 adds missing tests. Block 6 patches documentation.

**Tech Stack:** Rust, tokio, futures, parking_lot, reqwest, prometheus, cucumber BDD

---

### Task 1: Fix nofailover tag not filtered in select_candidates

**Files:**
- Modify: `src/pool/failover.rs:233-244` (filter predicate)
- Modify: `src/pool/failover.rs:256-363` (add unit tests)

- [ ] **Step 1: Add unit test for nofailover filtering**

In `src/pool/failover.rs`, inside `mod tests`, add:

```rust
#[test]
fn select_candidates_filters_nofailover() {
    let mut nofailover =
        make_member("pg-nofailover", Role::Replica, "streaming", "10.0.0.1", 5432);
    nofailover.tags.nofailover = true;

    let normal = make_member("pg-normal", Role::Replica, "streaming", "10.0.0.2", 5432);

    let candidates = select_candidates(&[nofailover, normal]);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].0, "10.0.0.2");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib select_candidates_filters_nofailover -- --nocapture`
Expected: FAIL — nofailover member is not filtered, candidates.len() == 2

- [ ] **Step 3: Add nofailover to filter predicate**

In `src/pool/failover.rs`, change `select_candidates`:

```rust
.filter(|m| {
    let alive = m.state == "streaming" || m.state == "running";
    alive && !m.tags.noloadbalance && !m.tags.archive && !m.tags.nofailover
})
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib select_candidates_filters_nofailover -- --nocapture`
Expected: PASS

---

### Task 2: Fix race in try_connect_candidates — leader blocks replica

**Files:**
- Modify: `src/pool/failover.rs:169-226` (try_connect_candidates)
- Modify: `src/pool/failover.rs` tests section (add unit tests)

- [ ] **Step 1: Add unit tests for candidate selection priority**

In `src/pool/failover.rs`, inside `mod tests`, add:

```rust
#[test]
fn select_candidates_all_replicas_no_sync() {
    let members = vec![
        make_member("pg-r1", Role::Replica, "streaming", "10.0.0.1", 5432),
        make_member("pg-r2", Role::Replica, "streaming", "10.0.0.2", 5432),
    ];
    let candidates = select_candidates(&members);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].2, Role::Replica);
}

#[test]
fn select_candidates_only_leader() {
    let members = vec![make_member(
        "pg-leader",
        Role::Leader,
        "running",
        "10.0.0.1",
        5432,
    )];
    let candidates = select_candidates(&members);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].2, Role::Leader);
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib select_candidates_all_replicas_no_sync select_candidates_only_leader -- --nocapture`
Expected: PASS (these test select_candidates, which already works correctly)

- [ ] **Step 3: Fix try_connect_candidates — track best with role**

In `src/pool/failover.rs`, replace the `try_connect_candidates` method. The key change: store `best: Option<(String, u16, Role)>` and replace it when a candidate with better (lower) priority arrives. Also replace `tokio::spawn` with pinned futures to avoid leaked tasks:

```rust
async fn try_connect_candidates(
    &self,
    candidates: &[(String, u16, Role)],
    timeout: Duration,
) -> Result<(String, u16), String> {
    let futs: Vec<_> = candidates
        .iter()
        .map(|(host, port, role)| {
            let addr = format!("{}:{}", host, port);
            let host = host.clone();
            let port = *port;
            let role = role.clone();
            Box::pin(async move {
                match tokio::time::timeout(timeout, TcpStream::connect(&addr)).await {
                    Ok(Ok(_stream)) => {
                        debug!("failover: TCP connect ok to {} (role: {:?})", addr, role);
                        Some((host, port, role))
                    }
                    Ok(Err(e)) => {
                        warn!("failover: TCP connect failed to {}: {}", addr, e);
                        None
                    }
                    Err(_) => {
                        warn!("failover: TCP connect timeout to {}", addr);
                        None
                    }
                }
            })
        })
        .collect();

    let mut best: Option<(String, u16, Role)> = None;
    let mut remaining = futs;

    while !remaining.is_empty() {
        let (result, _idx, rest) = futures::future::select_all(remaining).await;

        if let Some((host, port, role)) = result {
            if role == Role::SyncStandby {
                // sync_standby is top priority — return immediately,
                // dropping `rest` cancels remaining futures
                return Ok((host, port));
            }
            let dominated = best
                .as_ref()
                .map_or(true, |(_, _, best_role)| role_priority(&role) < role_priority(best_role));
            if dominated {
                best = Some((host, port, role));
            }
        }

        remaining = rest;
    }

    best.map(|(h, p, _)| (h, p))
        .ok_or_else(|| "all candidates unreachable".to_string())
}
```

- [ ] **Step 4: Run all failover tests**

Run: `cargo test --lib failover -- --nocapture`
Expected: all PASS

- [ ] **Step 5: Run cargo clippy**

Run: `cargo clippy -- --deny warnings`
Expected: no warnings

---

### Task 3: Switch to parking_lot::Mutex, add BlacklistCheck, fix gauge, fix whitelist invalidation

This task replaces `std::sync::Mutex` with `parking_lot::Mutex` (no poisoning), changes `is_blacklisted()` to return a `BlacklistCheck` enum that signals when blacklist just expired (so the caller can bump epoch and clear gauge), and adds automatic whitelist clearing on blacklist expiry and on fallback connection failure.

**Files:**
- Modify: `src/pool/failover.rs` (full rewrite of state management)
- Modify: `src/pool/server_pool.rs:145-315` (handle BlacklistCheck in create())

- [ ] **Step 1: Add BlacklistCheck enum and rewrite FailoverState**

In `src/pool/failover.rs`, replace the imports and struct:

```rust
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::future::Shared;
use futures::FutureExt;
use log::{debug, info, warn};
use parking_lot::Mutex;
use tokio::net::TcpStream;

use crate::patroni::client::PatroniClient;
use crate::patroni::types::{ClusterResponse, Member, Role};

type SharedClusterFuture =
    Shared<Pin<Box<dyn Future<Output = Result<ClusterResponse, String>> + Send>>>;

pub struct FallbackTarget {
    pub host: String,
    pub port: u16,
    pub lifetime_ms: u64,
}

#[derive(Debug, PartialEq)]
pub enum BlacklistCheck {
    /// Not blacklisted (never was, or manually cleared)
    NotBlacklisted,
    /// Blacklist is still active
    Active,
    /// Was blacklisted, just expired on this check
    JustExpired,
}

pub struct FailoverState {
    blacklisted_until: Mutex<Option<Instant>>,
    whitelisted_host: Mutex<Option<(String, u16)>>,
    inflight: tokio::sync::Mutex<Option<(Instant, SharedClusterFuture)>>,

    /// Suppresses repeated info logs while blacklisted
    blacklist_logged: AtomicBool,

    pool_name: String,
    discovery_urls: Vec<String>,
    blacklist_duration: Duration,
    connect_timeout: Duration,
    server_lifetime_ms: u64,

    /// Reusable HTTP client for Patroni API
    patroni_client: PatroniClient,
}
```

- [ ] **Step 2: Rewrite FailoverState::new and is_blacklisted**

```rust
impl FailoverState {
    pub fn new(
        pool_name: String,
        discovery_urls: Vec<String>,
        blacklist_duration: Duration,
        connect_timeout: Duration,
        request_timeout: Duration,
        server_lifetime_ms: u64,
    ) -> Self {
        Self {
            blacklisted_until: Mutex::new(None),
            whitelisted_host: Mutex::new(None),
            inflight: tokio::sync::Mutex::new(None),
            blacklist_logged: AtomicBool::new(false),
            pool_name,
            discovery_urls,
            blacklist_duration,
            connect_timeout,
            server_lifetime_ms,
            patroni_client: PatroniClient::new(request_timeout, connect_timeout),
        }
    }

    pub fn check_blacklist(&self) -> BlacklistCheck {
        let mut guard = self.blacklisted_until.lock();
        match *guard {
            Some(until) => {
                if Instant::now() < until {
                    BlacklistCheck::Active
                } else {
                    // Blacklist just expired — clean up
                    *guard = None;
                    drop(guard);

                    // Clear whitelist — topology may have changed
                    {
                        let mut wl = self.whitelisted_host.lock();
                        *wl = None;
                    }

                    // Reset gauge
                    crate::prometheus::FAILOVER_HOST_BLACKLISTED
                        .with_label_values(&[&self.pool_name])
                        .set(0.0);

                    // Reset log suppression
                    self.blacklist_logged.store(false, Ordering::Relaxed);

                    // Clear fallback host metric
                    let _ = crate::prometheus::FAILOVER_FALLBACK_HOST
                        .remove_label_values(&[&self.pool_name]);

                    BlacklistCheck::JustExpired
                }
            }
            None => BlacklistCheck::NotBlacklisted,
        }
    }

    pub fn blacklist(&self) {
        let mut guard = self.blacklisted_until.lock();
        *guard = Some(Instant::now() + self.blacklist_duration);
    }

    pub fn clear_whitelist(&self) {
        let mut guard = self.whitelisted_host.lock();
        *guard = None;
        let _ = crate::prometheus::FAILOVER_FALLBACK_HOST
            .remove_label_values(&[&self.pool_name]);
    }

    /// Returns true if this is the first log since blacklist was set
    pub fn should_log_blacklist(&self) -> bool {
        !self.blacklist_logged.swap(true, Ordering::Relaxed)
    }
```

- [ ] **Step 3: Rewrite get_fallback_target and fetch_cluster_coalesced**

```rust
    pub async fn get_fallback_target(&self) -> Result<FallbackTarget, String> {
        // 1. Check whitelist — return cached host immediately
        {
            let guard = self.whitelisted_host.lock();
            if let Some((ref host, port)) = *guard {
                debug!("failover: returning whitelisted host {}:{}", host, port);
                crate::prometheus::FAILOVER_WHITELIST_HITS_TOTAL
                    .with_label_values(&[&self.pool_name])
                    .inc();
                return Ok(FallbackTarget {
                    host: host.clone(),
                    port,
                    lifetime_ms: self.server_lifetime_ms,
                });
            }
        }

        // 2. Fetch /cluster via coalesced request
        let cluster = self.fetch_cluster_coalesced().await?;

        // 3. Filter and sort candidates
        let candidates = select_candidates(&cluster.members);
        if candidates.is_empty() {
            return Err(format!(
                "no eligible members in /cluster response ({} total, all filtered)",
                cluster.members.len()
            ));
        }

        // 4. Parallel TCP connect to all candidates
        let (host, port) = self.try_connect_candidates(&candidates, self.connect_timeout).await?;

        // 5. Whitelist the successful host
        {
            let mut guard = self.whitelisted_host.lock();
            *guard = Some((host.clone(), port));
        }
        info!(
            "[{}] failover: whitelisted {}:{}",
            self.pool_name, host, port
        );

        // 6. Set fallback host metric
        crate::prometheus::FAILOVER_FALLBACK_HOST
            .with_label_values(&[&self.pool_name, &host, &port.to_string()])
            .set(1.0);

        Ok(FallbackTarget {
            host,
            port,
            lifetime_ms: self.server_lifetime_ms,
        })
    }

    async fn fetch_cluster_coalesced(&self) -> Result<ClusterResponse, String> {
        // Inflight coalescing: reuse existing request if it was created < 1s ago
        let shared = {
            let mut guard = self.inflight.lock().await;
            let now = Instant::now();
            if let Some((created, ref shared)) = *guard {
                if now.duration_since(created) < Duration::from_secs(1) {
                    shared.clone()
                } else {
                    // Stale — create new request
                    let shared = self.make_cluster_future();
                    *guard = Some((now, shared.clone()));
                    // Only count actual HTTP requests, not coalesced joins
                    crate::prometheus::FAILOVER_DISCOVERY_TOTAL
                        .with_label_values(&[&self.pool_name])
                        .inc();
                    shared
                }
            } else {
                let shared = self.make_cluster_future();
                *guard = Some((now, shared.clone()));
                crate::prometheus::FAILOVER_DISCOVERY_TOTAL
                    .with_label_values(&[&self.pool_name])
                    .inc();
                shared
            }
        };

        let start = Instant::now();
        let result = shared.await;

        crate::prometheus::FAILOVER_DISCOVERY_DURATION
            .with_label_values(&[&self.pool_name])
            .observe(start.elapsed().as_secs_f64());

        result
    }

    fn make_cluster_future(&self) -> SharedClusterFuture {
        let urls = self.discovery_urls.clone();
        let client = self.patroni_client.clone();
        let fut = async move { client.fetch_cluster(&urls).await.map_err(|e| e.to_string()) };
        fut.boxed().shared()
    }
```

Note: `make_cluster_future` requires `PatroniClient` to be `Clone`. We add `#[derive(Clone)]` to it — `reqwest::Client` is already cheaply cloneable (Arc internally).

- [ ] **Step 4: Update PatroniClient to derive Clone**

In `src/patroni/client.rs`, add `Clone`:

```rust
#[derive(Clone)]
pub struct PatroniClient {
    http: reqwest::Client,
}
```

Remove `request_timeout` field from `FailoverState` (it's now inside `PatroniClient`).

- [ ] **Step 5: Update ServerPool::create to handle BlacklistCheck**

In `src/pool/server_pool.rs`, replace the blacklist check block (lines 156-182):

```rust
        // If primary is blacklisted, skip directly to fallback
        if let Some(ref failover) = self.failover_state {
            match failover.check_blacklist() {
                super::failover::BlacklistCheck::Active => {
                    let log_level = if failover.should_log_blacklist() {
                        log::Level::Info
                    } else {
                        log::Level::Debug
                    };
                    match failover.get_fallback_target().await {
                        Ok(target) => {
                            log::log!(
                                log_level,
                                "[{}@{}] failover: primary blacklisted, connecting to {}:{}",
                                self.address.username, self.address.pool_name,
                                target.host, target.port,
                            );
                            crate::prometheus::FAILOVER_CONNECTIONS_TOTAL
                                .with_label_values(&[&self.address.pool_name])
                                .inc();
                            return self.create_fallback_connection(target).await;
                        }
                        Err(e) => {
                            warn!(
                                "[{}@{}] failover: discovery failed while blacklisted: {e}",
                                self.address.username, self.address.pool_name,
                            );
                            crate::prometheus::FAILOVER_DISCOVERY_ERRORS_TOTAL
                                .with_label_values(&[&self.address.pool_name])
                                .inc();
                            // Fall through to try primary anyway
                        }
                    }
                }
                super::failover::BlacklistCheck::JustExpired => {
                    // Blacklist expired — bump epoch to drain stale fallback connections
                    info!(
                        "[{}@{}] failover: blacklist expired, returning to primary, draining fallback connections",
                        self.address.username, self.address.pool_name,
                    );
                    self.bump_epoch();
                }
                super::failover::BlacklistCheck::NotBlacklisted => {
                    // Normal path — no failover needed
                }
            }
        }
```

- [ ] **Step 6: Handle fallback connection failure — clear whitelist**

In `src/pool/server_pool.rs`, in the error branch of `create()` (lines 276-314), after the `failover.get_fallback_target()` Ok branch, add whitelist clearing on fallback connection failure. Replace the `create_fallback_connection` call in both places (blacklisted path and initial-error path):

In the initial-error path (the Err(err) branch around line 296):

```rust
                            Ok(target) => {
                                info!(
                                    "[{}@{}] failover: connecting to {}:{} (original error: {err})",
                                    self.address.username,
                                    self.address.pool_name,
                                    target.host,
                                    target.port,
                                );
                                crate::prometheus::FAILOVER_CONNECTIONS_TOTAL
                                    .with_label_values(&[&self.address.pool_name])
                                    .inc();
                                let result = self.create_fallback_connection(target).await;
                                if result.is_err() {
                                    failover.clear_whitelist();
                                }
                                return result;
                            }
```

And similarly in the blacklisted path:
```rust
                        Ok(target) => {
                            log::log!(
                                log_level,
                                "[{}@{}] failover: primary blacklisted, connecting to {}:{}",
                                self.address.username, self.address.pool_name,
                                target.host, target.port,
                            );
                            crate::prometheus::FAILOVER_CONNECTIONS_TOTAL
                                .with_label_values(&[&self.address.pool_name])
                                .inc();
                            let result = self.create_fallback_connection(target).await;
                            if result.is_err() {
                                failover.clear_whitelist();
                            }
                            return result;
                        }
```

- [ ] **Step 7: Remove dead code clear_failover from ServerPool**

Delete the `clear_failover()` method from `src/pool/server_pool.rs` (lines 322-327).

- [ ] **Step 8: Update unit test for blacklist lifecycle**

In `src/pool/failover.rs`, update the `blacklist_lifecycle` test:

```rust
#[test]
fn blacklist_lifecycle() {
    let state = FailoverState::new(
        "test_pool".to_string(),
        vec![],
        Duration::from_secs(10),
        Duration::from_secs(1),
        Duration::from_secs(2),
        30_000,
    );

    // Initially not blacklisted
    assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);

    // After blacklist() — is active
    state.blacklist();
    assert_eq!(state.check_blacklist(), BlacklistCheck::Active);
}
```

- [ ] **Step 9: Add test for blacklist expiry clearing whitelist**

```rust
#[test]
fn blacklist_expiry_clears_whitelist() {
    let state = FailoverState::new(
        "test_pool".to_string(),
        vec![],
        Duration::from_millis(1), // 1ms blacklist — expires almost immediately
        Duration::from_secs(1),
        Duration::from_secs(2),
        30_000,
    );

    // Set whitelist manually
    {
        let mut guard = state.whitelisted_host.lock();
        *guard = Some(("10.0.0.5".to_string(), 5432));
    }

    state.blacklist();
    std::thread::sleep(Duration::from_millis(10));

    // Blacklist expired — should return JustExpired and clear whitelist
    assert_eq!(state.check_blacklist(), BlacklistCheck::JustExpired);

    // Whitelist should be cleared
    let guard = state.whitelisted_host.lock();
    assert!(guard.is_none());

    // Subsequent check should be NotBlacklisted
    assert_eq!(state.check_blacklist(), BlacklistCheck::NotBlacklisted);
}
```

- [ ] **Step 10: Run all tests and clippy**

Run: `cargo test --lib failover -- --nocapture && cargo clippy -- --deny warnings`
Expected: all PASS, no warnings

- [ ] **Step 11: Commit block 1+2+3 (bugs + architecture + reliability)**

```bash
git add src/pool/failover.rs src/pool/server_pool.rs src/patroni/client.rs
git commit -m "Fix failover bugs: nofailover filter, candidate race, gauge reset, whitelist invalidation, leaked tasks, parking_lot, log flooding"
```

---

### Task 4: Apply fallback lifetime to connections

**Files:**
- Modify: `src/server/server_backend.rs:44-163` (add override_lifetime_ms field)
- Modify: `src/server/server_backend.rs:960-979` (set field in startup)
- Modify: `src/pool/server_pool.rs:329-367` (set override in create_fallback_connection)
- Modify: `src/pool/inner.rs:324-338` (use override in new_object_inner)

- [ ] **Step 1: Add override_lifetime_ms field to Server**

In `src/server/server_backend.rs`, after `close_reason` (line 162), add:

```rust
    /// Overrides pool-level lifetime for this connection (milliseconds).
    /// Used by failover connections that need a shorter lifetime to return
    /// to primary sooner.
    pub(crate) override_lifetime_ms: Option<u64>,
```

- [ ] **Step 2: Initialize the field in Server::startup**

In `src/server/server_backend.rs`, in the Server struct literal (around line 978, after `close_reason: None,`), add:

```rust
                        override_lifetime_ms: None,
```

- [ ] **Step 3: Set override in create_fallback_connection**

In `src/pool/server_pool.rs`, in `create_fallback_connection`, after `Ok(conn)` (around line 358):

```rust
            Ok(mut conn) => {
                conn.stats.idle(0);
                conn.override_lifetime_ms = Some(target.lifetime_ms);
                Ok(conn)
            }
```

- [ ] **Step 4: Use override in new_object_inner**

In `src/pool/inner.rs`, change `new_object_inner` (lines 324-338):

```rust
    fn new_object_inner(
        &self,
        obj: Server,
        coordinator_permit: Option<pool_coordinator::CoordinatorPermit>,
    ) -> ObjectInner {
        let lifetime_ms = obj
            .override_lifetime_ms
            .unwrap_or(self.server_pool.lifetime_ms());
        ObjectInner {
            obj,
            metrics: Metrics::new(
                lifetime_ms,
                self.server_pool.idle_timeout_ms(),
                self.server_pool.current_epoch(),
            ),
            coordinator_permit,
        }
    }
```

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test && cargo clippy -- --deny warnings`
Expected: all PASS, no warnings

- [ ] **Step 6: Commit**

```bash
git add src/server/server_backend.rs src/pool/inner.rs src/pool/server_pool.rs
git commit -m "Apply failover_server_lifetime to fallback connections"
```

---

### Task 5: Add config validation for failover parameters

**Files:**
- Modify: `src/config/pool.rs:227-346` (Pool::validate)

- [ ] **Step 1: Add validation rules**

In `src/config/pool.rs`, inside `validate()`, before the `Ok(())` at line 345, add:

```rust
        // Validate failover discovery settings
        if let Some(ref urls) = self.patroni_discovery_urls {
            if urls.is_empty() {
                return Err(Error::BadConfig(
                    "patroni_discovery_urls cannot be an empty list; \
                     remove the setting to disable failover discovery"
                        .into(),
                ));
            }
            for url in urls {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(Error::BadConfig(format!(
                        "patroni_discovery_urls: invalid URL '{}'; \
                         must start with http:// or https://",
                        url
                    )));
                }
            }
        }

        if let Some(ref dur) = self.failover_blacklist_duration {
            if dur.is_zero() {
                return Err(Error::BadConfig(
                    "failover_blacklist_duration must be > 0".into(),
                ));
            }
        }
```

- [ ] **Step 2: Run tests and clippy**

Run: `cargo test && cargo clippy -- --deny warnings`
Expected: all PASS

- [ ] **Step 3: Commit**

```bash
git add src/config/pool.rs
git commit -m "Add config validation for failover discovery parameters"
```

---

### Task 6: Add missing Prometheus metrics

**Files:**
- Modify: `src/prometheus/mod.rs` (add new metrics)
- Modify: `src/pool/failover.rs` (use new metrics — already done in Task 3)

- [ ] **Step 1: Add FAILOVER_FALLBACK_HOST and FAILOVER_WHITELIST_HITS_TOTAL**

In `src/prometheus/mod.rs`, after `FAILOVER_DISCOVERY_DURATION` block, add:

```rust
pub(crate) static FAILOVER_FALLBACK_HOST: Lazy<GaugeVec> = Lazy::new(|| {
    let gauge = GaugeVec::new(
        Opts::new(
            "pg_doorman_failover_fallback_host",
            "Currently whitelisted fallback host (1 = active), by pool, host, port.",
        ),
        &["pool", "host", "port"],
    )
    .unwrap();
    REGISTRY.register(Box::new(gauge.clone())).unwrap();
    gauge
});

pub(crate) static FAILOVER_WHITELIST_HITS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "pg_doorman_failover_whitelist_hits_total",
            "Total whitelist cache hits (fallback host reused without new discovery), by pool.",
        ),
        &["pool"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});
```

- [ ] **Step 2: Run tests and clippy**

Run: `cargo test && cargo clippy -- --deny warnings`
Expected: all PASS

- [ ] **Step 3: Commit**

```bash
git add src/prometheus/mod.rs
git commit -m "Add failover_fallback_host and whitelist_hits metrics"
```

---

### Task 7: Add BDD tests

**Files:**
- Modify: `tests/bdd/features/patroni-failover-discovery.feature`
- Modify: `tests/bdd/mock_patroni_helper.rs` (may need a step for stopping individual mock servers)

- [ ] **Step 1: Add BDD scenario — multiple Patroni URLs, first dead**

Append to `tests/bdd/features/patroni-failover-discovery.feature`:

```gherkin
  Scenario: Discovery succeeds via second Patroni URL when first is unreachable
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'patroni1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node2",
            "host": "127.0.0.1",
            "port": ${PG_PORT},
            "role": "sync_standby",
            "state": "streaming",
            "timeline": 1,
            "lag": 0
          }
        ]
      }
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      connect_timeout = "5s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_discovery_urls = ["http://127.0.0.1:59997", "http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      failover_blacklist_duration = "5s"
      failover_connect_timeout = "3s"
      failover_discovery_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"
```

- [ ] **Step 2: Add BDD scenario — all TCP candidates unreachable**

```gherkin
  Scenario: Connection fails when all cluster members are unreachable via TCP
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'patroni1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": 59996,
            "role": "sync_standby",
            "state": "streaming",
            "timeline": 1
          },
          {
            "name": "node2",
            "host": "127.0.0.1",
            "port": 59995,
            "role": "replica",
            "state": "streaming",
            "timeline": 1
          }
        ]
      }
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      connect_timeout = "5s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_discovery_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      failover_blacklist_duration = "5s"
      failover_connect_timeout = "3s"
      failover_discovery_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails
```

- [ ] **Step 3: Add BDD scenario — dynamic member list update mid-test**

```gherkin
  Scenario: Doorman uses updated member list after mock Patroni response changes
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And mock Patroni server 'patroni1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": 59999,
            "role": "leader",
            "state": "stopped",
            "timeline": 1
          },
          {
            "name": "node2",
            "host": "127.0.0.1",
            "port": 59998,
            "role": "sync_standby",
            "state": "streaming",
            "timeline": 1
          }
        ]
      }
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      connect_timeout = "5s"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = 59999
      patroni_discovery_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
      failover_blacklist_duration = "2s"
      failover_connect_timeout = "3s"
      failover_discovery_timeout = "3s"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    # First attempt fails — node2:59998 is not a real PG
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails
    # Update mock to point to real PG
    When mock Patroni server 'patroni1' response is updated to:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": 59999,
            "role": "leader",
            "state": "stopped",
            "timeline": 1
          },
          {
            "name": "node2",
            "host": "127.0.0.1",
            "port": ${PG_PORT},
            "role": "sync_standby",
            "state": "streaming",
            "timeline": 1
          }
        ]
      }
      """
    # Wait for blacklist to expire (2s + margin)
    And I wait 3 seconds
    Then psql query "SELECT 1" via pg_doorman as user "example_user_1" to database "example_db" with password "" returns "1"
```

- [ ] **Step 4: Check if "I wait N seconds" step exists**

Search for existing wait/sleep step definitions. If not found, add one in `tests/bdd/mock_patroni_helper.rs`:

```rust
#[when(regex = r"^I wait (\d+) seconds?$")]
pub async fn wait_seconds(_world: &mut DoormanWorld, seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}
```

- [ ] **Step 5: Run BDD tests locally**

Run: `cargo test --test bdd -- --tags @patroni_failover --nocapture`
Expected: all scenarios PASS

- [ ] **Step 6: Commit**

```bash
git add tests/bdd/features/patroni-failover-discovery.feature tests/bdd/mock_patroni_helper.rs
git commit -m "Add BDD tests: multi-URL discovery, unreachable candidates, dynamic member update"
```

---

### Task 8: Update documentation

**Files:**
- Modify: `documentation/en/src/tutorials/patroni-failover-discovery.md`
- Modify: `documentation/ru/patroni-failover-discovery.md`

- [ ] **Step 1: Add sections to EN docs**

In `documentation/en/src/tutorials/patroni-failover-discovery.md`, before the "## Relationship to patroni_proxy" section, add:

```markdown
## Active transactions

If PostgreSQL crashes while a client is in the middle of a transaction,
the client receives a connection error. doorman does not migrate
in-flight transactions to a fallback host — the client must retry.

New queries from the same or other clients go through the fallback path
automatically.

## Operational notes

**Credentials.** All cluster nodes must accept the same username and
password that doorman uses. Patroni clusters typically share
`pg_hba.conf` via bootstrap configuration, but this is not guaranteed.
Verify that fallback nodes accept the configured credentials.

**TLS.** Fallback connections use the same `server_tls_mode` as the
primary. If the primary uses a unix socket (no TLS), fallback TCP
connections will also run without TLS. Configure `server_tls_mode`
explicitly if fallback connections must be encrypted.

**DNS.** Use IP addresses in `patroni_discovery_urls`, not hostnames.
DNS resolution failure during a failover adds latency and may cause
discovery to fail entirely.

**standby_leader.** Patroni standby clusters use the `standby_leader`
role. doorman treats it as "other" (lowest priority, after sync_standby
and replica). This is correct for most deployments.
```

- [ ] **Step 2: Add equivalent sections to RU docs**

In `documentation/ru/patroni-failover-discovery.md`, before "## Связь с patroni_proxy", add:

```markdown
## Активные транзакции

Если PostgreSQL падает во время транзакции клиента, клиент получает
ошибку соединения. doorman не переносит незавершённые транзакции
на fallback-хост — клиент должен выполнить retry.

Новые запросы от этого и других клиентов автоматически идут через
fallback.

## Эксплуатационные заметки

**Credentials.** Все узлы кластера должны принимать те же username
и password, которые использует doorman. Patroni-кластеры обычно
разделяют `pg_hba.conf` через bootstrap-конфигурацию, но это не
гарантировано. Убедитесь, что fallback-узлы принимают настроенные
credentials.

**TLS.** Fallback-соединения используют тот же `server_tls_mode`,
что и primary. Если primary использует unix socket (без TLS),
fallback TCP-соединения тоже пойдут без TLS. Настройте
`server_tls_mode` явно, если fallback-соединения должны быть
зашифрованы.

**DNS.** Используйте IP-адреса в `patroni_discovery_urls`, а не
hostname. Неудача DNS-резолва во время failover добавляет задержку
и может привести к полному отказу discovery.

**standby_leader.** В standby-кластерах Patroni используется роль
`standby_leader`. doorman обрабатывает её как "other" (наименьший
приоритет, после sync_standby и replica). Для большинства
развёртываний это корректно.
```

- [ ] **Step 3: Commit**

```bash
git add documentation/
git commit -m "Document active transactions, credentials, TLS, DNS, standby_leader"
```

---

### Task 9: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: all PASS

- [ ] **Step 2: Run clippy and fmt**

Run: `cargo fmt && cargo clippy -- --deny warnings`
Expected: clean

- [ ] **Step 3: Verify diff looks correct**

Run: `git log --oneline master..HEAD` to review commit history.
Run: `git diff master..HEAD --stat` to verify changed files.
