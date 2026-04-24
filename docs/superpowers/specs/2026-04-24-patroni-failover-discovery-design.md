# Patroni failover discovery: design spec

## Goal

When the local PostgreSQL dies (Patroni switchover, crash, OOM), doorman
queries the Patroni REST API `/cluster`, finds a live cluster member,
and routes new connections there for ~30 seconds. Short-term bridge,
not a full failover mechanism.

Detailed context, research, and rationale: `~/Projects/pg_doorman_failover.md`
and `~/Projects/pg_doorman_failover_ru.md`.


## Module structure

```
src/patroni/
    mod.rs          -- pub mod client; pub mod types;
    types.rs        -- Member, ClusterResponse, Role
    client.rs       -- PatroniClient: parallel fetch /cluster

src/pool/
    failover.rs     -- FailoverState: blacklist, whitelist, coalescing,
                       member selection
    server_pool.rs  -- modify create(): check FailoverState
    mod.rs          -- wire config into ServerPool

src/config/
    pool.rs         -- new fields: patroni_discovery_urls, failover_*

src/app/
    errors.rs       -- new variants: ConnectError, ServerUnavailableError
```


## src/patroni/types.rs

```rust
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
```

`Role::Other(String)` for unknown roles -- forward compatible with
future Patroni versions.


## src/patroni/client.rs

```rust
pub struct PatroniClient {
    http: reqwest::Client,
}

impl PatroniClient {
    pub fn new(
        request_timeout: Duration,
        connect_timeout: Duration,
    ) -> Self;

    /// Fetch /cluster from all URLs in parallel.
    /// Returns first successful response, aborts the rest.
    pub async fn fetch_cluster(
        &self,
        urls: &[String],
    ) -> Result<ClusterResponse, PatroniError>;
}
```

Implementation of `fetch_cluster`:
1. `tokio::spawn` per URL: `self.http.get(format!("{url}/cluster")).send()`
2. `tokio::select!` across all -- first HTTP 200 with valid JSON wins
3. Remaining tasks get `abort()`
4. All failed: `PatroniError::AllUrlsFailed(Vec<(String, String)>)`

`reqwest::Client` created with:
- `timeout(request_timeout)` -- full request timeout (DNS + connect + body)
- `connect_timeout(connect_timeout)` -- separate, shorter, prevents DNS hangs
- `no_proxy()` -- ignore `HTTP_PROXY` env var
- Single instance, reuses connections across calls

This module is the DevOps review focus. Scope: accept URLs, return
members or error. No pool logic, no blacklist, no failover decisions.


## src/pool/failover.rs

```rust
pub struct FailoverState {
    blacklisted_until: Mutex<Option<Instant>>,
    whitelisted_host: Mutex<Option<(String, u16)>>,
    inflight: Mutex<Option<Shared<Pin<Box<dyn Future<...>>>>>>,

    patroni_client: PatroniClient,
    discovery_urls: Vec<String>,
    blacklist_duration: Duration,
    connect_timeout: Duration,
    server_lifetime: Duration,

    // Prometheus counters/gauges/histogram
    discovery_total: IntCounter,
    connections_total: IntCounter,
    discovery_errors_total: IntCounter,
    host_blacklisted: IntGauge,
    discovery_duration: Histogram,
}
```

Public API:

```rust
impl FailoverState {
    pub fn is_blacklisted(&self) -> bool;
    pub fn blacklist(&self);
    pub fn clear(&self);  // called on SIGHUP reload

    /// Get fallback target. Checks whitelist first, then fetches /cluster.
    pub async fn get_fallback_target(&self) -> Result<FallbackTarget, Error>;
}

pub struct FallbackTarget {
    pub host: String,
    pub port: u16,
    pub lifetime: Duration,
}
```

`get_fallback_target` logic:
1. Whitelist exists? Return it (no HTTP, no parallel connect).
2. No whitelist: coalesced fetch `/cluster` via `Shared<Future>` pattern.
   First caller creates the future, others clone and await same result.
3. Filter members: `state == "streaming"` or `state == "running"`.
4. Parallel TCP connect to all filtered members.
   `sync_standby` has priority: if only replica responds, wait up to
   2 seconds for sync_standby before taking replica.
   Overall timeout: `connect_timeout`.
5. First successful TCP connect: whitelist it, return `FallbackTarget`.
6. All failed: error.

Whitelist and blacklist reset together (after `blacklist_duration` or
on reload).


## Error variants

Add to `src/app/errors.rs`:

```rust
pub enum Error {
    // ... existing ...

    /// TCP or Unix socket connect() failed. Backend process unreachable.
    /// Only produced by create_unix_stream_inner / create_tcp_stream_inner.
    ConnectError(String),

    /// PG responded with FATAL, SQLSTATE class 57P (operator intervention):
    /// 57P01 admin_shutdown, 57P02 crash_shutdown,
    /// 57P03 cannot_connect_now (starting up).
    /// Backend accepted the connection but is not serving queries.
    ServerUnavailableError(String, ServerIdentifier),
}
```

Changes:
- `stream.rs:149`: `SocketError` -> `ConnectError` in `create_unix_stream_inner`
- `stream.rs:170`: `SocketError` -> `ConnectError` in `create_tcp_stream_inner`
- `startup_error.rs:50`: if `PgErrorMsg.code` starts with `"57P"` ->
  `ServerUnavailableError` instead of `ServerStartupError`

Failover trigger classification:

```rust
fn is_backend_unreachable(err: &Error) -> bool {
    matches!(err, Error::ConnectError(_) | Error::ServerUnavailableError(_, _))
}
```

No string parsing. `ConnectError` = OS-level (socket not found,
connection refused, connect timeout). `ServerUnavailableError` =
PG-level (shutting down, starting up). Everything else (auth errors,
protocol errors) does NOT trigger failover.


## ServerPool::create() modification

`ServerPool` gets a new field: `failover_state: Option<Arc<FailoverState>>`.
`None` when `patroni_discovery_urls` is not configured -- zero overhead.

Modified flow:

```
create()
  |
  +-- failover_state exists AND is_blacklisted()?
  |    YES --> get_fallback_target()
  |             OK --> Server::startup() with target host:port
  |                    and reduced lifetime
  |             FAIL --> return error
  |    NO --> continue to normal path
  |
  +-- normal path: Server::startup() to local host
  |    OK --> return Server
  |    Err where is_backend_unreachable() -->
  |         failover_state exists?
  |           YES --> blacklist(), get_fallback_target(), retry
  |           NO --> return error
  |    Err (other) --> return error (auth, protocol -- PG is alive)
```

Fallback Server::startup() reuses the same `address` (same credentials,
same database), only `host` and `port` differ. Lifetime in `Metrics`
is set to `failover_state.server_lifetime` (default 30s) instead of
normal `self.lifetime_ms`.


## Configuration

New fields in `src/config/pool.rs`:

```rust
pub patroni_discovery_urls: Option<Vec<String>>,
pub failover_blacklist_duration: Option<Duration>,   // "30s"
pub failover_discovery_timeout: Option<Duration>,    // "5s"
pub failover_connect_timeout: Option<Duration>,      // "5s"
pub failover_server_lifetime: Option<Duration>,      // default = blacklist
```

Uses existing `Duration` type with human parsing (`"30s"`, `"5m"`).

YAML example:
```yaml
pools:
  mydb:
    server_host: "/var/run/postgresql"
    server_port: 5432
    patroni_discovery_urls:
      - "http://10.0.0.1:8008"
      - "http://10.0.0.2:8008"
    failover_blacklist_duration: "30s"
```

Reload (SIGHUP): if `patroni_discovery_urls` changed, recreate
`FailoverState`. Always call `clear()` to reset blacklist/whitelist.


## Prometheus metrics

```
pg_doorman_failover_discovery_total{pool}         -- counter
pg_doorman_failover_connections_total{pool}        -- counter
pg_doorman_failover_discovery_errors_total{pool}   -- counter
pg_doorman_failover_host_blacklisted{pool,host}    -- gauge
pg_doorman_failover_discovery_duration_seconds{pool} -- histogram
```


## Testing

### Unit tests

**src/patroni/types.rs:**
- Deserialize valid `/cluster` JSON
- Empty members list
- Unknown role -> `Role::Other`
- Missing optional fields (`lag`, `api_url`)

**src/pool/failover.rs:**
- `is_blacklisted()`: before/after expiry
- `clear()`: resets blacklist + whitelist
- Member selection: sync_standby before replica, filter by state

### BDD tests

Reuse mock Patroni HTTP server pattern from
`patroni_proxy/tests/bdd/mock_patroni_helper.rs`.
Copy and adapt for doorman World (not extract, per design decision).

Feature file style matches patroni_proxy -- inline JSON:

```gherkin
Scenario: doorman routes to sync_standby when local PG is down
  Given mock Patroni server 'patroni1' with response:
    """
    {
      "scope": "test_cluster",
      "members": [
        {
          "name": "node1", "host": "127.0.0.1", "port": ${DEAD_PG_PORT},
          "role": "leader", "state": "running", "timeline": 1
        },
        {
          "name": "node2", "host": "127.0.0.1", "port": ${PG_PORT},
          "role": "sync_standby", "state": "streaming", "timeline": 1
        }
      ]
    }
    """
  And pg_doorman is configured with patroni_discovery_urls
  And the local PostgreSQL is stopped
  When the client executes "SELECT 1"
  Then the query succeeds
```

BDD scenarios to cover:
1. Switchover: query succeeds via fallback
2. sync_standby preferred over async replica
3. Return to original host after blacklist expires
4. Concurrent failures: single /cluster request
5. Fallback connections respect coordinator limits
6. All Patroni URLs unreachable: error
7. Auth error does not trigger discovery
8. Fallback server expires via reduced lifetime
9. Whitelist reused without /cluster
10. Whitelist host dies: re-fetch /cluster


## Decisions log

1. Module location: `src/patroni/` (not separate crate, not inside pool)
2. HTTP library: reqwest (already in deps)
3. Request coalescing: `Shared<Future>` pattern (Mutex<Option<Shared<...>>>)
4. Failover state: separate `FailoverState` struct, not fields in ServerPool
5. Error classification: new Error variants (`ConnectError`,
   `ServerUnavailableError`), not string parsing
6. Own Member struct, do not extract from patroni_proxy
7. Reload resets blacklist and whitelist
8. `patroni_discovery_role` is overengineering, not implementing
9. Parallel polling of Patroni URLs: fire-and-take-first
10. Parallel TCP connect to members, sync_standby priority with 2s grace
11. Reduced `failover_server_lifetime` for fallback connections
12. Duration config uses existing human parsing ("30s", "5m")
