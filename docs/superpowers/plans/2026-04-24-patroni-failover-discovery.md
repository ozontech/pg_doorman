# Patroni Failover Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When local PostgreSQL dies, doorman queries Patroni `/cluster` API to find a live cluster member and temporarily routes connections there (~30s).

**Architecture:** New module `src/patroni/` (HTTP client + types) consumed by `src/pool/failover.rs` (blacklist/whitelist/coalescing state). `ServerPool::create()` checks `FailoverState` on connection errors. Two new `Error` variants (`ConnectError`, `ServerUnavailableError`) replace string-based error classification.

**Tech Stack:** reqwest (already in deps), tokio, serde, futures (`Shared`), prometheus

**Spec:** `docs/superpowers/specs/2026-04-24-patroni-failover-discovery-design.md`

---

### Task 1: Error variants — ConnectError and ServerUnavailableError

**Files:**
- Modify: `src/app/errors.rs`
- Modify: `src/server/stream.rs:144-173` (unix + tcp connect)
- Modify: `src/server/startup_error.rs:40-55` (SQLSTATE 57P check)
- Test: `cargo test` + `cargo clippy`

- [ ] **Step 1: Add ConnectError variant to Error enum**

In `src/app/errors.rs`, add after `SocketError(String)`:

```rust
/// TCP or Unix socket connect() failed. Backend process unreachable.
ConnectError(String),
```

Add Display arm after `SocketError` arm:

```rust
Error::ConnectError(msg) => write!(f, "Backend connect error: {msg}"),
```

- [ ] **Step 2: Add ServerUnavailableError variant to Error enum**

In `src/app/errors.rs`, add after `ServerAuthError`:

```rust
/// PG startup FATAL with SQLSTATE class 57P (operator intervention):
/// 57P01 admin_shutdown, 57P02 crash_shutdown, 57P03 cannot_connect_now.
ServerUnavailableError(String, ServerIdentifier),
```

Add Display arm:

```rust
Error::ServerUnavailableError(error, server_identifier) => write!(
    f,
    "Backend unavailable: {error} for {server_identifier}"
),
```

- [ ] **Step 3: Change create_unix_stream_inner to return ConnectError**

In `src/server/stream.rs:144-153`, change:

```rust
// Before:
return Err(Error::SocketError(format!(
    "Failed to connect to Unix socket {host}:{port}: {err}"
)));
// After:
return Err(Error::ConnectError(format!(
    "Failed to connect to Unix socket {host}:{port}: {err}"
)));
```

- [ ] **Step 4: Change create_tcp_stream_inner to return ConnectError**

In `src/server/stream.rs:166-173`, change:

```rust
// Before:
return Err(Error::SocketError(format!(
    "Could not connect to {host}:{port}: {err}"
)));
// After:
return Err(Error::ConnectError(format!(
    "Could not connect to {host}:{port}: {err}"
)));
```

- [ ] **Step 5: Handle SQLSTATE 57P in startup_error.rs**

In `src/server/startup_error.rs`, inside the `Ok(f)` branch (line ~50), before the existing `Err(Error::ServerStartupError(...))`:

```rust
Ok(f) => {
    error!(
        "[{}@{}] startup error: severity={}, code={}, message={}",
        server_identifier.username,
        server_identifier.pool_name,
        f.severity,
        f.code,
        f.message
    );
    if f.code.starts_with("57P") {
        Err(Error::ServerUnavailableError(
            f.message,
            server_identifier.clone(),
        ))
    } else {
        Err(Error::ServerStartupError(
            f.message,
            server_identifier.clone(),
        ))
    }
}
```

- [ ] **Step 6: Fix any compile errors from new variants**

Run: `cargo build 2>&1 | head -50`

Check for exhaustive match patterns in `src/client/error_handling.rs` and elsewhere that match on `Error`. Add arms for `ConnectError` and `ServerUnavailableError` — handle them the same as `SocketError` and `ServerStartupError` respectively.

- [ ] **Step 7: Run tests and clippy**

Run: `cargo test 2>&1 | tail -20`
Run: `cargo clippy -- --deny "warnings" 2>&1 | tail -20`
Expected: both pass

- [ ] **Step 8: Commit**

```
git add src/app/errors.rs src/server/stream.rs src/server/startup_error.rs src/client/
git commit -m "Add ConnectError and ServerUnavailableError variants

Structured error classification for failover trigger detection.
ConnectError: OS-level connect failure (socket not found, refused).
ServerUnavailableError: PG FATAL with SQLSTATE 57P (shutdown, starting up).
No string parsing needed to distinguish 'backend dead' from 'bad credentials'."
```

---

### Task 2: Patroni types — Member, Role, ClusterResponse

**Files:**
- Create: `src/patroni/mod.rs`
- Create: `src/patroni/types.rs`
- Modify: `src/main.rs` or `src/lib.rs` — add `mod patroni;`
- Test: unit tests in `src/patroni/types.rs`

- [ ] **Step 1: Find the crate root and check module structure**

Run: `head -30 src/lib.rs 2>/dev/null || head -30 src/main.rs`

Determine where to add `pub mod patroni;`.

- [ ] **Step 2: Create src/patroni/mod.rs**

```rust
pub mod client;
pub mod types;
```

- [ ] **Step 3: Create src/patroni/types.rs with types and unit tests**

```rust
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ClusterResponse {
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    pub name: String,
    #[serde(deserialize_with = "deserialize_role")]
    pub role: Role,
    pub state: String,
    pub host: String,
    pub port: u16,
    pub api_url: Option<String>,
    pub lag: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Leader,
    SyncStandby,
    Replica,
    Other(String),
}

fn deserialize_role<'de, D>(deserializer: D) -> Result<Role, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(match s.as_str() {
        "leader" => Role::Leader,
        "sync_standby" => Role::SyncStandby,
        "replica" => Role::Replica,
        other => Role::Other(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_cluster_response() {
        let json = r#"{
            "members": [
                {
                    "name": "node1",
                    "role": "leader",
                    "state": "running",
                    "host": "10.0.0.1",
                    "port": 5432,
                    "api_url": "http://10.0.0.1:8008/patroni",
                    "timeline": 1
                },
                {
                    "name": "node2",
                    "role": "sync_standby",
                    "state": "streaming",
                    "host": "10.0.0.2",
                    "port": 5432,
                    "lag": 0
                }
            ]
        }"#;
        let resp: ClusterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.members.len(), 2);
        assert_eq!(resp.members[0].role, Role::Leader);
        assert_eq!(resp.members[0].host, "10.0.0.1");
        assert_eq!(resp.members[1].role, Role::SyncStandby);
        assert_eq!(resp.members[1].lag, Some(0));
        assert!(resp.members[0].lag.is_none());
    }

    #[test]
    fn parse_empty_members() {
        let json = r#"{"members": []}"#;
        let resp: ClusterResponse = serde_json::from_str(json).unwrap();
        assert!(resp.members.is_empty());
    }

    #[test]
    fn parse_unknown_role() {
        let json = r#"{
            "members": [{
                "name": "n1", "role": "standby_leader",
                "state": "running", "host": "h", "port": 5432
            }]
        }"#;
        let resp: ClusterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.members[0].role,
            Role::Other("standby_leader".to_string())
        );
    }

    #[test]
    fn parse_missing_optional_fields() {
        let json = r#"{
            "members": [{
                "name": "n1", "role": "replica",
                "state": "streaming", "host": "h", "port": 5432
            }]
        }"#;
        let resp: ClusterResponse = serde_json::from_str(json).unwrap();
        assert!(resp.members[0].api_url.is_none());
        assert!(resp.members[0].lag.is_none());
    }
}
```

- [ ] **Step 4: Add mod patroni to crate root**

Add `pub mod patroni;` to the crate root file found in step 1.

- [ ] **Step 5: Run tests**

Run: `cargo test patroni::types 2>&1 | tail -20`
Expected: 4 tests pass

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -- --deny "warnings" 2>&1 | tail -20`
Expected: pass

- [ ] **Step 7: Commit**

```
git add src/patroni/
git commit -m "Add Patroni API types: Member, Role, ClusterResponse

Deserialization for /cluster endpoint JSON. Role enum with
Other(String) fallback for forward compatibility."
```

---

### Task 3: Patroni HTTP client — parallel fetch

**Files:**
- Create: `src/patroni/client.rs`
- Modify: `Cargo.toml` — add `futures` dependency if not present

- [ ] **Step 1: Check if futures crate is available**

Run: `grep "^futures" Cargo.toml`

If not present, add `futures = "0.3"` to `[dependencies]`.

- [ ] **Step 2: Create src/patroni/client.rs**

```rust
use std::time::Duration;

use log::{debug, error, warn};
use tokio::task::JoinHandle;

use super::types::ClusterResponse;

#[derive(Debug)]
pub enum PatroniError {
    AllUrlsFailed(Vec<(String, String)>),
}

impl std::fmt::Display for PatroniError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatroniError::AllUrlsFailed(errors) => {
                write!(f, "all patroni urls failed:")?;
                for (url, err) in errors {
                    write!(f, " {url}: {err};")?;
                }
                Ok(())
            }
        }
    }
}

pub struct PatroniClient {
    http: reqwest::Client,
}

impl PatroniClient {
    pub fn new(request_timeout: Duration, connect_timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .connect_timeout(connect_timeout)
            .no_proxy()
            .build()
            .expect("failed to build reqwest client");
        Self { http }
    }

    /// Fetch /cluster from all URLs in parallel.
    /// Returns first successful response, aborts the rest.
    pub async fn fetch_cluster(
        &self,
        urls: &[String],
    ) -> Result<ClusterResponse, PatroniError> {
        if urls.is_empty() {
            return Err(PatroniError::AllUrlsFailed(vec![]));
        }

        let mut handles: Vec<(String, JoinHandle<Result<ClusterResponse, String>>)> =
            Vec::with_capacity(urls.len());

        for url in urls {
            let request_url = format!("{}/cluster", url.trim_end_matches('/'));
            let http = self.http.clone();
            let url_for_log = url.clone();

            let handle = tokio::spawn(async move {
                debug!("fetching /cluster from {}", request_url);
                let resp = http
                    .get(&request_url)
                    .send()
                    .await
                    .map_err(|e| format!("{e}"))?;

                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status()));
                }

                resp.json::<ClusterResponse>()
                    .await
                    .map_err(|e| format!("json parse: {e}"))
            });

            handles.push((url_for_log, handle));
        }

        // Wait for first success, collect errors from failures
        let mut errors: Vec<(String, String)> = Vec::new();
        let mut remaining = handles;

        while !remaining.is_empty() {
            let (result, index, rest) = futures::future::select_all(
                remaining.into_iter().map(|(url, h)| {
                    Box::pin(async move { (url, h.await) })
                }),
            )
            .await;

            let (url, join_result) = result;

            match join_result {
                Ok(Ok(cluster)) => {
                    debug!(
                        "got /cluster from {}: {} members",
                        url,
                        cluster.members.len()
                    );
                    // Abort remaining tasks
                    for (_, (_, handle)) in rest.into_iter().enumerate() {
                        handle.1.abort();
                    }
                    return Ok(cluster);
                }
                Ok(Err(e)) => {
                    warn!("patroni url {} failed: {}", url, e);
                    errors.push((url, e));
                }
                Err(e) => {
                    warn!("patroni url {} task failed: {}", url, e);
                    errors.push((url, e.to_string()));
                }
            }

            remaining = rest.into_iter().map(|(_, v)| v).collect();
        }

        error!("all patroni discovery urls failed");
        Err(PatroniError::AllUrlsFailed(errors))
    }
}
```

- [ ] **Step 3: Run build**

Run: `cargo build 2>&1 | tail -20`
Expected: compiles

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- --deny "warnings" 2>&1 | tail -20`
Expected: pass

- [ ] **Step 5: Commit**

```
git add src/patroni/client.rs Cargo.toml
git commit -m "Add PatroniClient: parallel /cluster fetch

Queries all configured Patroni URLs simultaneously, takes the first
successful response, aborts the rest. Separate connect_timeout and
request_timeout. no_proxy() to ignore HTTP_PROXY env."
```

---

### Task 4: Configuration — new pool fields

**Files:**
- Modify: `src/config/pool.rs`

- [ ] **Step 1: Add failover fields to Pool struct**

In `src/config/pool.rs`, add before the `auth_query` field (before the TOML compatibility warning comment about complex objects):

```rust
    /// Patroni REST API URLs for failover discovery.
    /// When the local backend is unreachable, doorman queries /cluster
    /// to find an alternative. Feature is disabled when not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patroni_discovery_urls: Option<Vec<String>>,

    /// How long a failed host stays blacklisted. Default: "30s".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_blacklist_duration: Option<crate::config::Duration>,

    /// HTTP timeout for Patroni API requests. Default: "5s".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_discovery_timeout: Option<crate::config::Duration>,

    /// TCP connect timeout for fallback servers. Default: "5s".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_connect_timeout: Option<crate::config::Duration>,

    /// Lifetime for fallback connections. Default: same as blacklist duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_server_lifetime: Option<crate::config::Duration>,
```

- [ ] **Step 2: Add defaults to Pool::default() or equivalent**

Check if there's a `Default` impl or similar. If the struct uses `#[serde(default = "...")]` per field, no default() change needed — `Option` fields default to `None`. Verify the `hash_value` still works (fields are `Hash`-compatible — `Vec<String>` and `Duration` both implement `Hash`).

- [ ] **Step 3: Verify config parses with and without new fields**

Run: `cargo test config 2>&1 | tail -20`
Run: `cargo build 2>&1 | tail -10`

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- --deny "warnings" 2>&1 | tail -20`

- [ ] **Step 5: Commit**

```
git add src/config/pool.rs
git commit -m "Add failover discovery config fields to Pool

patroni_discovery_urls, failover_blacklist_duration,
failover_discovery_timeout, failover_connect_timeout,
failover_server_lifetime. All optional, feature disabled by default."
```

---

### Task 5: FailoverState — blacklist, whitelist, coalescing

**Files:**
- Create: `src/pool/failover.rs`
- Modify: `src/pool/mod.rs` — add `pub mod failover;`

- [ ] **Step 1: Create src/pool/failover.rs with FailoverState struct**

```rust
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::future::Shared;
use futures::FutureExt;
use log::{debug, info, warn};
use tokio::net::TcpStream;

use crate::patroni::client::{PatroniClient, PatroniError};
use crate::patroni::types::{ClusterResponse, Member, Role};

pub struct FallbackTarget {
    pub host: String,
    pub port: u16,
    pub lifetime_ms: u64,
}

pub struct FailoverState {
    blacklisted_until: Mutex<Option<Instant>>,
    whitelisted_host: Mutex<Option<(String, u16)>>,
    inflight: tokio::sync::Mutex<
        Option<Shared<Pin<Box<dyn std::future::Future<Output = Result<ClusterResponse, String>> + Send>>>>,
    >,

    patroni_client: PatroniClient,
    discovery_urls: Vec<String>,
    blacklist_duration: Duration,
    connect_timeout: Duration,
    server_lifetime_ms: u64,
}

impl FailoverState {
    pub fn new(
        discovery_urls: Vec<String>,
        blacklist_duration: Duration,
        discovery_timeout: Duration,
        connect_timeout: Duration,
        server_lifetime_ms: u64,
    ) -> Self {
        let patroni_client = PatroniClient::new(discovery_timeout, connect_timeout);
        Self {
            blacklisted_until: Mutex::new(None),
            whitelisted_host: Mutex::new(None),
            inflight: tokio::sync::Mutex::new(None),
            patroni_client,
            discovery_urls,
            blacklist_duration,
            connect_timeout,
            server_lifetime_ms,
        }
    }

    pub fn is_blacklisted(&self) -> bool {
        let guard = self.blacklisted_until.lock().unwrap();
        guard.map_or(false, |until| Instant::now() < until)
    }

    pub fn blacklist(&self) {
        let mut guard = self.blacklisted_until.lock().unwrap();
        *guard = Some(Instant::now() + self.blacklist_duration);
        info!(
            "failover: blacklisted local host for {}ms",
            self.blacklist_duration.as_millis()
        );
    }

    pub fn clear(&self) {
        *self.blacklisted_until.lock().unwrap() = None;
        *self.whitelisted_host.lock().unwrap() = None;
        // inflight future completes naturally, no need to abort
        info!("failover: cleared blacklist and whitelist");
    }

    pub async fn get_fallback_target(&self) -> Result<FallbackTarget, String> {
        // 1. Check whitelist
        {
            let guard = self.whitelisted_host.lock().unwrap();
            if let Some((ref host, port)) = *guard {
                debug!("failover: using whitelisted host {}:{}", host, port);
                return Ok(FallbackTarget {
                    host: host.clone(),
                    port,
                    lifetime_ms: self.server_lifetime_ms,
                });
            }
        }

        // 2. Fetch /cluster (coalesced)
        let cluster = self.fetch_cluster_coalesced().await?;

        // 3. Filter and sort members
        let candidates = Self::select_candidates(&cluster);
        if candidates.is_empty() {
            return Err("no suitable members in /cluster response".to_string());
        }

        // 4. Parallel connect, sync_standby priority
        let target = self.parallel_connect(&candidates).await?;

        // 5. Whitelist
        {
            let mut guard = self.whitelisted_host.lock().unwrap();
            *guard = Some((target.host.clone(), target.port));
        }
        info!(
            "failover: whitelisted {}:{} for fallback",
            target.host, target.port
        );

        Ok(target)
    }

    async fn fetch_cluster_coalesced(&self) -> Result<ClusterResponse, String> {
        // Check for inflight request
        let shared = {
            let mut guard = self.inflight.lock().await;
            if let Some(ref shared) = *guard {
                shared.clone()
            } else {
                let urls = self.discovery_urls.clone();
                let client_urls = urls;
                let patroni = &self.patroni_client;

                // Create the future
                let urls_for_fetch = self.discovery_urls.clone();
                let request_timeout = self.connect_timeout;
                let connect_timeout = self.connect_timeout;

                // We need to create a new PatroniClient for the spawned task
                // because &self cannot be sent across spawn boundary.
                let patroni_client = PatroniClient::new(
                    request_timeout,
                    connect_timeout,
                );
                let fut = async move {
                    patroni_client
                        .fetch_cluster(&urls_for_fetch)
                        .await
                        .map_err(|e| e.to_string())
                };
                let shared = fut.boxed().shared();
                *guard = Some(shared.clone());
                shared
            }
        };

        let result = shared.await;

        // Clear inflight after completion
        {
            let mut guard = self.inflight.lock().await;
            *guard = None;
        }

        result
    }

    fn select_candidates(cluster: &ClusterResponse) -> Vec<&Member> {
        let mut sync: Vec<&Member> = Vec::new();
        let mut replicas: Vec<&Member> = Vec::new();
        let mut others: Vec<&Member> = Vec::new();

        for member in &cluster.members {
            let alive = member.state == "streaming" || member.state == "running";
            if !alive {
                continue;
            }
            match member.role {
                Role::SyncStandby => sync.push(member),
                Role::Replica => replicas.push(member),
                _ => others.push(member),
            }
        }

        let mut result = Vec::new();
        result.extend(sync);
        result.extend(replicas);
        result.extend(others);
        result
    }

    async fn parallel_connect(
        &self,
        candidates: &[&Member],
    ) -> Result<FallbackTarget, String> {
        use tokio::time::{sleep, timeout};

        let timeout_dur = self.connect_timeout;
        let mut handles = Vec::new();

        for member in candidates {
            let host = member.host.clone();
            let port = member.port;
            let role = member.role.clone();

            let handle = tokio::spawn(async move {
                match TcpStream::connect(format!("{}:{}", host, port)).await {
                    Ok(_stream) => {
                        // Drop the stream — ServerPool::create() will make the real connection
                        Ok((host, port, role))
                    }
                    Err(e) => Err(format!("{}:{}: {}", host, port, e)),
                }
            });
            handles.push(handle);
        }

        // Wait with overall timeout
        let result = timeout(timeout_dur, async {
            let mut first_replica: Option<(String, u16)> = None;
            let mut errors: Vec<String> = Vec::new();
            let mut remaining = handles;

            while !remaining.is_empty() {
                let (result, _index, rest) =
                    futures::future::select_all(remaining).await;

                match result {
                    Ok(Ok((host, port, role))) => {
                        if role == Role::SyncStandby {
                            // Abort remaining
                            for h in rest {
                                h.abort();
                            }
                            return Ok(FallbackTarget {
                                host,
                                port,
                                lifetime_ms: self.server_lifetime_ms,
                            });
                        }
                        // Replica responded — remember but wait for sync
                        if first_replica.is_none() {
                            first_replica = Some((host, port));
                        }
                    }
                    Ok(Err(e)) => errors.push(e),
                    Err(e) => errors.push(e.to_string()),
                }

                remaining = rest;
            }

            // No sync_standby responded. Use replica if available.
            if let Some((host, port)) = first_replica {
                return Ok(FallbackTarget {
                    host,
                    port,
                    lifetime_ms: self.server_lifetime_ms,
                });
            }

            Err(format!("all members unreachable: {:?}", errors))
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err("failover connect timeout".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_lifecycle() {
        let state = FailoverState::new(
            vec![],
            Duration::from_millis(100),
            Duration::from_secs(5),
            Duration::from_secs(5),
            30000,
        );

        assert!(!state.is_blacklisted());
        state.blacklist();
        assert!(state.is_blacklisted());
        state.clear();
        assert!(!state.is_blacklisted());
    }

    #[test]
    fn select_candidates_ordering() {
        let cluster = ClusterResponse {
            members: vec![
                Member {
                    name: "n1".into(), role: Role::Replica,
                    state: "streaming".into(), host: "h1".into(), port: 5432,
                    api_url: None, lag: None,
                },
                Member {
                    name: "n2".into(), role: Role::SyncStandby,
                    state: "streaming".into(), host: "h2".into(), port: 5432,
                    api_url: None, lag: None,
                },
                Member {
                    name: "n3".into(), role: Role::Leader,
                    state: "stopped".into(), host: "h3".into(), port: 5432,
                    api_url: None, lag: None,
                },
            ],
        };
        let candidates = FailoverState::select_candidates(&cluster);
        assert_eq!(candidates.len(), 2); // stopped leader filtered out
        assert_eq!(candidates[0].role, Role::SyncStandby);
        assert_eq!(candidates[1].role, Role::Replica);
    }

    #[test]
    fn select_candidates_empty() {
        let cluster = ClusterResponse { members: vec![] };
        let candidates = FailoverState::select_candidates(&cluster);
        assert!(candidates.is_empty());
    }
}
```

- [ ] **Step 2: Add mod failover to pool module**

In `src/pool/mod.rs`, add:

```rust
pub mod failover;
```

- [ ] **Step 3: Run tests**

Run: `cargo test failover 2>&1 | tail -20`
Expected: 3 tests pass

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- --deny "warnings" 2>&1 | tail -20`

- [ ] **Step 5: Commit**

```
git add src/pool/failover.rs src/pool/mod.rs
git commit -m "Add FailoverState: blacklist, whitelist, parallel connect

Blacklist/whitelist lifecycle, member selection (sync_standby first),
parallel TCP connect with sync priority, request coalescing via
Shared<Future>."
```

---

### Task 6: Wire FailoverState into ServerPool::create()

**Files:**
- Modify: `src/pool/server_pool.rs` — add failover_state field, modify create()
- Modify: `src/pool/mod.rs` — pass config to ServerPool construction

- [ ] **Step 1: Add failover_state field to ServerPool**

In `src/pool/server_pool.rs`, add field to `ServerPool` struct:

```rust
    /// Failover state for Patroni discovery. None if not configured.
    failover_state: Option<Arc<super::failover::FailoverState>>,
```

Add `use std::sync::Arc;` if not present.

- [ ] **Step 2: Add is_backend_unreachable helper**

In `src/pool/server_pool.rs`, add:

```rust
/// Check if an error indicates the backend is unreachable (not misconfigured).
fn is_backend_unreachable(err: &Error) -> bool {
    matches!(
        err,
        Error::ConnectError(_) | Error::ServerUnavailableError(_, _)
    )
}
```

- [ ] **Step 3: Modify create() — add fallback on error**

In `ServerPool::create()`, after the existing error handling (the `Err(err)` arm at line ~242), add failover logic. The modified match block:

```rust
match result {
    Ok(conn) => {
        conn.stats.idle(0);
        Ok(conn)
    }
    Err(err) => {
        active_stats.disconnect();
        // Failover: if backend unreachable and failover configured
        if is_backend_unreachable(&err) {
            if let Some(ref failover) = self.failover_state {
                failover.blacklist();
                match failover.get_fallback_target().await {
                    Ok(target) => {
                        info!(
                            "failover: connecting to {}:{} instead of {}:{}",
                            target.host, target.port,
                            self.address.host, self.address.port,
                        );
                        return self.create_fallback_connection(target).await;
                    }
                    Err(e) => {
                        warn!("failover: discovery failed: {e}");
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Err(err)
    }
}
```

- [ ] **Step 4: Add create() early check for blacklisted host**

At the beginning of `create()`, after semaphore acquire, before `Server::startup()`:

```rust
// If primary is blacklisted, go straight to fallback
if let Some(ref failover) = self.failover_state {
    if failover.is_blacklisted() {
        match failover.get_fallback_target().await {
            Ok(target) => {
                info!(
                    "failover: primary blacklisted, connecting to {}:{}",
                    target.host, target.port,
                );
                return self.create_fallback_connection(target).await;
            }
            Err(e) => {
                warn!("failover: discovery failed while blacklisted: {e}");
                // Fall through to try primary anyway
            }
        }
    }
}
```

- [ ] **Step 5: Add create_fallback_connection method**

```rust
async fn create_fallback_connection(
    &self,
    target: super::failover::FallbackTarget,
) -> Result<Server, Error> {
    let mut fallback_address = self.address.clone();
    fallback_address.host = target.host;
    fallback_address.port = target.port;

    let stats = Arc::new(ServerStats::new(
        fallback_address.clone(),
        crate::utils::clock::now(),
    ));
    stats.register(stats.clone());

    let result = Server::startup(
        &fallback_address,
        &self.user,
        &self.database,
        self.client_server_map.clone(),
        stats.clone(),
        self.cleanup_connections,
        self.log_client_parameter_status_changes,
        self.prepared_statement_cache_size,
        self.application_name.clone(),
        self.session_mode,
    )
    .await;

    match result {
        Ok(conn) => {
            conn.stats.idle(0);
            Ok(conn)
        }
        Err(err) => {
            stats.disconnect();
            Err(err)
        }
    }
}
```

Note: the reduced lifetime (`target.lifetime_ms`) needs to be applied to the `Metrics` of the returned `Server`. This depends on how `Metrics` is constructed in the pool — find where `lifetime_ms` is set on `Metrics` and apply `target.lifetime_ms` for fallback connections. This may require passing lifetime as parameter to `create_fallback_connection` and overriding on the returned `Server` object.

- [ ] **Step 6: Wire FailoverState in pool construction**

Find where `ServerPool` is constructed in `src/pool/mod.rs` (in `ConnectionPool::from_config()` or similar). Add logic:

```rust
let failover_state = pool_config
    .patroni_discovery_urls
    .as_ref()
    .map(|urls| {
        let blacklist_dur = pool_config
            .failover_blacklist_duration
            .map(|d| std::time::Duration::from_millis(d.0))
            .unwrap_or(std::time::Duration::from_secs(30));
        let discovery_timeout = pool_config
            .failover_discovery_timeout
            .map(|d| std::time::Duration::from_millis(d.0))
            .unwrap_or(std::time::Duration::from_secs(5));
        let connect_timeout = pool_config
            .failover_connect_timeout
            .map(|d| std::time::Duration::from_millis(d.0))
            .unwrap_or(std::time::Duration::from_secs(5));
        let lifetime = pool_config
            .failover_server_lifetime
            .map(|d| d.0)
            .unwrap_or(blacklist_dur.as_millis() as u64);

        Arc::new(super::failover::FailoverState::new(
            urls.clone(),
            blacklist_dur,
            discovery_timeout,
            connect_timeout,
            lifetime,
        ))
    });
```

Pass `failover_state` to `ServerPool` constructor.

- [ ] **Step 7: Handle reload — clear FailoverState**

In the reload path (where `ConnectionPool::from_config()` decides to reuse or recreate pools), if pool is reused and `failover_state` exists, call `failover_state.clear()`.

- [ ] **Step 8: Build and test**

Run: `cargo build 2>&1 | tail -20`
Run: `cargo clippy -- --deny "warnings" 2>&1 | tail -20`

- [ ] **Step 9: Commit**

```
git add src/pool/server_pool.rs src/pool/mod.rs
git commit -m "Wire FailoverState into ServerPool::create()

On ConnectError or ServerUnavailableError: blacklist primary,
query Patroni /cluster, connect to fallback. Blacklisted primary
skips straight to fallback on subsequent create() calls."
```

---

### Task 7: BDD tests — mock Patroni helper for doorman

**Files:**
- Create: `tests/bdd/mock_patroni_helper.rs`
- Modify: `tests/bdd/world.rs` — add mock state fields
- Modify: `tests/bdd/main.rs` — register module

- [ ] **Step 1: Study doorman World struct**

Read `tests/bdd/world.rs` to understand the existing World struct and how steps are registered. Read `tests/bdd/main.rs` for module structure.

- [ ] **Step 2: Add mock Patroni fields to doorman World**

Add to the World struct (same pattern as patroni_proxy World):

```rust
pub mock_patroni_shutdowns: HashMap<String, Arc<AtomicBool>>,
pub mock_patroni_ports: Vec<u16>,
pub mock_patroni_names: HashMap<String, u16>,
pub mock_patroni_responses: HashMap<String, Arc<RwLock<String>>>,
```

- [ ] **Step 3: Create tests/bdd/mock_patroni_helper.rs**

Copy and adapt from `src/bin/patroni_proxy/tests/bdd/mock_patroni_helper.rs`. Change the World type to doorman's World. Key step definitions:

- `Given mock Patroni server '{name}' with response:` (inline JSON)
- `When mock Patroni server '{name}' response is updated to:` (dynamic update)
- Shutdown on World drop

- [ ] **Step 4: Register module in main.rs**

Add `mod mock_patroni_helper;` to `tests/bdd/main.rs`.

- [ ] **Step 5: Build BDD tests**

Run: `cargo test --test bdd -- --tags "not @skip" 2>&1 | tail -20`
Expected: compiles, existing tests still pass

- [ ] **Step 6: Commit**

```
git add tests/bdd/mock_patroni_helper.rs tests/bdd/world.rs tests/bdd/main.rs
git commit -m "Add mock Patroni HTTP server helper for doorman BDD tests

Reusable step definitions for mock /cluster responses with
inline JSON, matching patroni_proxy test patterns."
```

---

### Task 8: BDD test — basic failover scenario

**Files:**
- Create: `tests/bdd/features/patroni-failover-discovery.feature`

- [ ] **Step 1: Write the basic switchover scenario**

```gherkin
@patroni_failover
Feature: Patroni failover discovery

  Scenario: doorman routes to fallback when local PG is down
    Given mock Patroni server 'patroni1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1", "host": "127.0.0.1", "port": 59999,
            "role": "leader", "state": "running", "timeline": 1
          },
          {
            "name": "node2", "host": "127.0.0.1", "port": ${PG_PORT},
            "role": "sync_standby", "state": "streaming", "timeline": 1, "lag": 0
          }
        ]
      }
      """
    And a]doorman config with patroni_discovery_urls = ["http://127.0.0.1:${PATRONI_PATRONI1_PORT}"]
    And the local server_host points to a non-existent unix socket
    When the client connects and executes "SELECT 1"
    Then the query succeeds
```

Note: exact step definition wording depends on existing doorman BDD infrastructure. The implementer must adapt step names to match existing patterns (study `tests/bdd/doorman_helper.rs` for available steps). Port 59999 is intentionally unreachable (simulates dead local PG).

- [ ] **Step 2: Add doorman config step for patroni_discovery_urls**

This requires a new step definition in `tests/bdd/doorman_helper.rs` (or a new helper) that configures doorman with the failover settings. Study how existing config is generated in BDD tests.

- [ ] **Step 3: Run BDD test**

Run: `cargo test --test bdd -- "patroni_failover" 2>&1 | tail -40`
Expected: scenario passes — client query succeeds through fallback

- [ ] **Step 4: Add more scenarios incrementally**

Add remaining scenarios from the spec one by one, running after each:
- sync_standby preferred over replica
- auth error does not trigger discovery
- all Patroni URLs unreachable
- return to original host after blacklist

Each scenario follows the same pattern: mock Patroni response → configure doorman → trigger condition → assert behavior.

- [ ] **Step 5: Commit**

```
git add tests/bdd/features/patroni-failover-discovery.feature tests/bdd/
git commit -m "Add BDD tests for Patroni failover discovery

Covers: basic failover routing, sync_standby priority,
auth error handling, unreachable URLs, blacklist expiry."
```

---

### Task 9: Prometheus metrics

**Files:**
- Modify: `src/pool/failover.rs` — add metric fields and recording
- Modify: `src/prometheus/` — register new metrics

- [ ] **Step 1: Study existing metrics registration**

Read `src/prometheus/mod.rs` and `src/prometheus/metrics.rs` to understand how metrics are registered and exposed.

- [ ] **Step 2: Add metric fields to FailoverState**

Add prometheus counter/gauge/histogram fields. Record in:
- `blacklist()` — increment `discovery_total`, set `host_blacklisted` to 1
- `clear()` — set `host_blacklisted` to 0
- `fetch_cluster_coalesced()` — observe `discovery_duration`, increment `discovery_errors_total` on failure
- `get_fallback_target()` success — increment `connections_total`

- [ ] **Step 3: Register metrics in prometheus module**

Add new metrics to the prometheus registration, with `pool` label.

- [ ] **Step 4: Run build and clippy**

Run: `cargo build 2>&1 | tail -10`
Run: `cargo clippy -- --deny "warnings" 2>&1 | tail -10`

- [ ] **Step 5: Commit**

```
git add src/pool/failover.rs src/prometheus/
git commit -m "Add Prometheus metrics for failover discovery

discovery_total, connections_total, discovery_errors_total,
host_blacklisted, discovery_duration_seconds."
```

---

### Task 10: Documentation update — config reference

**Files:**
- Modify: `documentation/en/src/tutorials/patroni-failover-discovery.md`
- Modify: `documentation/ru/patroni-failover-discovery.md`
- Modify: `pg_doorman.yaml` and/or `pg_doorman.toml` — add commented examples

- [ ] **Step 1: Add commented config example to pg_doorman.yaml**

In the pools section, add a commented block showing patroni_discovery_urls and failover parameters.

- [ ] **Step 2: Update documentation if any parameter names changed during implementation**

Review the implemented config field names against the docs. Fix any discrepancies.

- [ ] **Step 3: Commit**

```
git add documentation/ pg_doorman.yaml pg_doorman.toml
git commit -m "Update docs and example configs with failover discovery parameters"
```
