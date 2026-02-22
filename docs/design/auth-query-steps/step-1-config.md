# Step 1: AuthQueryConfig struct + get_pool_config()

## Goal

Add `AuthQueryConfig` to the config system and `get_pool_config()` to the pool
module. No behavioral changes — just data structures and access.

## 1.1 AuthQueryConfig struct

### File: `src/config/pool.rs`

Add new struct after `Pool`:

```rust
use crate::config::Duration;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthQueryConfig {
    /// SQL query to fetch credentials. Must return (username, password).
    /// Use $1 for the username parameter.
    pub query: String,

    /// PostgreSQL user for executor connections (runs auth queries).
    pub user: String,

    /// Password for executor user (plaintext).
    pub password: String,

    /// Database for executor connections (default: pool name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    /// Number of executor connections (default: 2). Opened eagerly at startup.
    #[serde(default = "AuthQueryConfig::default_pool_size")]
    pub pool_size: u32,

    /// Backend user for data connections. If set, all dynamic users share
    /// one pool with this identity (dedicated mode). If not set, each dynamic
    /// user gets their own pool (passthrough mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_user: Option<String>,

    /// Backend password for dedicated server_user (plaintext).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_password: Option<String>,

    /// Pool size for dynamic user data connections (default: 40).
    #[serde(default = "AuthQueryConfig::default_data_pool_size")]
    pub default_pool_size: u32,

    /// Max cache age for positive entries (default: "1h").
    /// Uses project's Duration type: supports "1h", "30s", "5m", or raw ms.
    #[serde(default = "AuthQueryConfig::default_cache_ttl")]
    pub cache_ttl: Duration,

    /// Cache TTL for "user not found" entries (default: "30s").
    #[serde(default = "AuthQueryConfig::default_cache_failure_ttl")]
    pub cache_failure_ttl: Duration,

    /// Min interval between re-fetches for same username on auth failure (default: "1s").
    #[serde(default = "AuthQueryConfig::default_min_interval")]
    pub min_interval: Duration,
}

impl AuthQueryConfig {
    fn default_pool_size() -> u32 { 2 }
    fn default_data_pool_size() -> u32 { 40 }
    fn default_cache_ttl() -> Duration { Duration::from_hours(1) }
    fn default_cache_failure_ttl() -> Duration { Duration::from_secs(30) }
    fn default_min_interval() -> Duration { Duration::from_secs(1) }

    /// Returns true if dedicated server_user mode is configured.
    pub fn is_dedicated_mode(&self) -> bool {
        self.server_user.is_some()
    }
}
```

**Note:** The project has `src/config/duration.rs` with a `Duration` type that supports
human-readable parsing (`"1h"`, `"30s"`, `"5m"`) as well as raw millisecond numbers.
This matches the design doc's config examples (`cache_ttl: "1h"`) and is consistent
with all other duration fields in the project (e.g., `connect_timeout`, `idle_timeout`).

Add field to `Pool` struct (BEFORE the `users` field, see TOML compatibility
comment on line 109-111):

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub auth_query: Option<AuthQueryConfig>,
```

Update `Pool::default()` to include `auth_query: None`.

### Validation in `Pool::validate()`

Add after username uniqueness check:

```rust
if let Some(ref aq) = self.auth_query {
    if aq.query.is_empty() {
        return Err(Error::BadConfig("auth_query.query cannot be empty".into()));
    }
    if aq.user.is_empty() {
        return Err(Error::BadConfig("auth_query.user cannot be empty".into()));
    }
    if aq.password.is_empty() {
        return Err(Error::BadConfig("auth_query.password cannot be empty".into()));
    }
    if aq.server_user.is_some() != aq.server_password.is_some() {
        return Err(Error::BadConfig(
            "auth_query: server_user and server_password must both be set or both omitted".into()
        ));
    }
    if aq.pool_size == 0 {
        return Err(Error::BadConfig("auth_query.pool_size must be > 0".into()));
    }
}
```

### Re-export

In `src/config/mod.rs`, add to the `pub use pool::` line:

```rust
pub use pool::{Pool, AuthQueryConfig};
```

## 1.2 get_pool_config()

### File: `src/pool/mod.rs`

Add new function after `get_pool()` (line ~520):

```rust
/// Get pool-level configuration by database name.
/// Returns the Pool config if any user pool exists for this database.
/// Used by auth_query to find auth_query config when user is not in static config.
pub fn get_pool_config(db: &str) -> Option<crate::config::Pool> {
    let config = crate::config::get_config();
    config.pools.get(db).cloned()
}
```

## 1.3 Unit tests

### File: `src/config/tests.rs` (existing test module)

Add tests:
- Parse YAML config with auth_query section (dedicated mode)
- Parse YAML config with auth_query section (passthrough mode)
- Parse YAML config without auth_query (backward compat)
- Validation: empty query → error
- Validation: server_user without server_password → error
- Validation: pool_size 0 → error
- Parse TOML config with auth_query section

## Checklist

- [ ] `AuthQueryConfig` struct with serde derives
- [ ] `auth_query: Option<AuthQueryConfig>` on `Pool`
- [ ] Default impls
- [ ] Validation in `Pool::validate()`
- [ ] Re-export in `config/mod.rs`
- [ ] `get_pool_config()` in `pool/mod.rs`
- [ ] Unit tests (7+)
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
- [ ] Existing tests pass
