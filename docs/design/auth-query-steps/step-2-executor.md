# Step 2: AuthQueryExecutor

## Goal

Create the executor pool that connects to PostgreSQL and runs auth queries.
Uses `deadpool-postgres` directly (Decision from Problem 7).

## Dependencies

- Step 1 (AuthQueryConfig struct)

## 2.1 New dependency

### File: `Cargo.toml`

Add `deadpool-postgres` and `tokio-postgres` (if not already present) to dependencies:

```toml
deadpool-postgres = { version = "0.14", features = ["rt_tokio_1"] }
tokio-postgres = "0.7"
```

Check if `deadpool-postgres` is already pulled in transitively. If the project
uses a custom pool implementation (it does — `src/pool/inner.rs`), we still
need `deadpool-postgres` as a separate lightweight pool for the executor.

**Alternative:** If adding `deadpool-postgres` is undesirable, use `tokio-postgres`
directly with a simple `Vec<Client>` + semaphore. The executor only needs 2
connections — a full pool library may be overkill. Decide during implementation.

## 2.2 AuthQueryExecutor struct

### File: `src/auth/auth_query.rs` (NEW)

```rust
use std::sync::Arc;
use log::{error, info, warn};
use tokio_postgres::{Client, NoTls, Row};

use crate::config::AuthQueryConfig;
use crate::errors::Error;

/// Executor pool for running auth_query SELECT statements.
/// Wraps a small number of persistent connections to auth_query.database.
pub struct AuthQueryExecutor {
    config: AuthQueryConfig,
    pool: deadpool_postgres::Pool,
    // Or: simple connection management with Vec<Arc<Mutex<Client>>>
}

impl AuthQueryExecutor {
    /// Create executor and establish connections eagerly.
    /// Called at pg_doorman startup. Connections MUST succeed before
    /// accepting client traffic (prevents max_connections deadlock).
    pub async fn new(
        config: &AuthQueryConfig,
        pool_name: &str,
        server_host: &str,
        server_port: u16,
    ) -> Result<Self, Error> {
        let database = config.database.clone()
            .unwrap_or_else(|| pool_name.to_string());

        let mut pg_config = tokio_postgres::Config::new();
        pg_config.host(server_host);
        pg_config.port(server_port);
        pg_config.user(&config.user);
        pg_config.password(&config.password);
        pg_config.dbname(&database);
        pg_config.connect_timeout(std::time::Duration::from_secs(5));

        // Build deadpool config
        let mgr = deadpool_postgres::Manager::from_config(
            pg_config,
            NoTls,  // TLS support can be added later based on pool TLS config
            deadpool_postgres::ManagerConfig {
                recycling_method: deadpool_postgres::RecyclingMethod::Fast,
            },
        );

        let pool = deadpool_postgres::Pool::builder(mgr)
            .max_size(config.pool_size as usize)
            .build()
            .map_err(|e| Error::BadConfig(format!(
                "Failed to create auth_query executor pool: {e}"
            )))?;

        // Eagerly verify at least one connection works
        let _conn = pool.get().await.map_err(|e| Error::BadConfig(format!(
            "auth_query executor failed to connect to {server_host}:{server_port}/{database} \
             as user '{}': {e}",
            config.user
        )))?;

        info!(
            "auth_query executor pool ready: {}@{server_host}:{server_port}/{database} \
             (pool_size={})",
            config.user, config.pool_size
        );

        Ok(Self {
            config: config.clone(),
            pool,
        })
    }

    /// Fetch password hash for a username from PostgreSQL.
    /// Returns Some((username, password_hash)) or None if user not found.
    /// Fails fast on SQL errors (never blocks — PgBouncer #649 lesson).
    pub async fn fetch_password(
        &self,
        username: &str,
    ) -> Result<Option<(String, String)>, Error> {
        let conn = self.pool.get().await.map_err(|e| {
            error!("auth_query executor: failed to get connection: {e}");
            Error::AuthQueryError(format!("executor connection unavailable: {e}"))
        })?;

        let rows = conn
            .query(&self.config.query, &[&username])
            .await
            .map_err(|e| {
                error!("auth_query execution failed for user '{username}': {e}");
                Error::AuthQueryError(format!("query execution failed: {e}"))
            })?;

        match rows.len() {
            0 => Ok(None),
            1 => {
                let row = &rows[0];
                let user: String = row.try_get(0).map_err(|e| {
                    Error::AuthQueryError(format!("failed to read username column: {e}"))
                })?;
                let password: Option<String> = row.try_get(1).map_err(|e| {
                    Error::AuthQueryError(format!("failed to read password column: {e}"))
                })?;
                match password {
                    Some(p) if !p.is_empty() => Ok(Some((user, p))),
                    _ => {
                        warn!("auth_query: user '{username}' has NULL or empty password");
                        Ok(None)  // Treat as "user not found" — no auth possible
                    }
                }
            }
            n => {
                warn!("auth_query returned {n} rows for user '{username}', using first");
                let row = &rows[0];
                let user: String = row.try_get(0)?;
                let password: Option<String> = row.try_get(1)?;
                match password {
                    Some(p) if !p.is_empty() => Ok(Some((user, p))),
                    _ => Ok(None),
                }
            }
        }
    }
}
```

### Error type

Add to `src/errors.rs`:

```rust
AuthQueryError(String),
```

with appropriate Display impl.

## 2.3 Module registration

### File: `src/auth/mod.rs`

Add at the top:

```rust
pub mod auth_query;
```

## 2.4 TLS consideration

The initial implementation uses `NoTls` for executor connections. If the pool
config has TLS enabled, this should be propagated. Can be deferred to a follow-up
since auth_query.database is typically on the same host/network as the data pools.

## Checklist

- [ ] Add `deadpool-postgres` / `tokio-postgres` dependency (or decide on simpler approach)
- [ ] Create `src/auth/auth_query.rs` with `AuthQueryExecutor`
- [ ] `new()` — eager connection, clear error on failure
- [ ] `fetch_password()` — parameterized query, fail fast
- [ ] Add `AuthQueryError` to `src/errors.rs`
- [ ] Register module in `src/auth/mod.rs`
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
- [ ] Existing tests pass
