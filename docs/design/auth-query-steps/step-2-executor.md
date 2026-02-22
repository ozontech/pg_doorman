# Step 2: AuthQueryExecutor

## Goal

Create the executor pool that connects to PostgreSQL and runs auth queries.
Uses `deadpool-postgres` directly (Decision from Problem 7).

## Dependencies

- Step 1 (AuthQueryConfig struct)

## 2.1 New dependency

### File: `Cargo.toml`

The project uses a custom pool implementation (`src/pool/inner.rs`) for data
connections. For the auth_query executor, we need a separate, lightweight pool
that connects to a DIFFERENT database with DIFFERENT credentials.

**Recommended approach: `tokio-postgres` + `Arc<Semaphore>`.**

The executor needs only 2 connections — adding a full pool framework is
overkill. A simple approach:

```toml
# tokio-postgres may already be a transitive dependency — check Cargo.lock.
# If not present, add explicitly:
tokio-postgres = "0.7"
```

The executor manages a fixed set of `tokio_postgres::Client` connections
behind an `Arc<Semaphore>` for concurrency control. This avoids adding
`deadpool-postgres` as a new dependency.

**Alternative:** If more sophisticated connection management is needed later
(recycling, health checks), `deadpool-postgres` can be added. But for 2
connections that are opened eagerly and kept alive, a semaphore is sufficient.

## 2.2 AuthQueryExecutor struct

### File: `src/auth/auth_query.rs` (NEW)

```rust
use std::sync::Arc;
use log::{error, info, warn};
use tokio::sync::{Semaphore, Mutex as TokioMutex};
use tokio_postgres::{Client, NoTls, Row};

use crate::config::AuthQueryConfig;
use crate::errors::Error;

/// Executor pool for running auth_query SELECT statements.
/// Wraps a small number of persistent connections to auth_query.database.
/// Uses tokio-postgres directly with a semaphore for concurrency control
/// (no external pool dependency needed for 2 connections).
pub struct AuthQueryExecutor {
    config: AuthQueryConfig,
    clients: Vec<Arc<TokioMutex<Client>>>,
    semaphore: Arc<Semaphore>,
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

        // Eagerly open all connections at startup
        let mut clients = Vec::with_capacity(config.pool_size as usize);
        for i in 0..config.pool_size {
            let (client, connection) = pg_config.connect(NoTls).await
                .map_err(|e| Error::BadConfig(format!(
                    "auth_query executor connection {i} failed to \
                     {server_host}:{server_port}/{database} as '{}': {e}",
                    config.user
                )))?;

            // Spawn the connection task (handles async protocol messages)
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    error!("auth_query executor connection lost: {e}");
                }
            });

            clients.push(Arc::new(TokioMutex::new(client)));
        }

        let semaphore = Arc::new(Semaphore::new(config.pool_size as usize));

        info!(
            "auth_query executor pool ready: {}@{server_host}:{server_port}/{database} \
             (pool_size={})",
            config.user, config.pool_size
        );

        Ok(Self {
            config: config.clone(),
            clients,
            semaphore,
        })
    }

    /// Fetch password hash for a username from PostgreSQL.
    /// Returns Some((username, password_hash)) or None if user not found.
    /// Fails fast on SQL errors (never blocks — PgBouncer #649 lesson).
    pub async fn fetch_password(
        &self,
        username: &str,
    ) -> Result<Option<(String, String)>, Error> {
        // Acquire semaphore permit (limits concurrency to pool_size)
        let _permit = self.semaphore.acquire().await.map_err(|e| {
            error!("auth_query executor: semaphore closed: {e}");
            Error::AuthQueryError(format!("executor unavailable: {e}"))
        })?;

        // Round-robin or find an unlocked client
        // (Simple approach: try each client, first unlockable one wins)
        let mut conn_guard = None;
        for client in &self.clients {
            if let Ok(guard) = client.try_lock() {
                conn_guard = Some(guard);
                break;
            }
        }
        let conn = conn_guard.ok_or_else(|| {
            error!("auth_query executor: all connections busy");
            Error::AuthQueryError("executor connections busy".into())
        })?;

        let rows = conn
            .query(&self.config.query, &[&username as &(dyn tokio_postgres::types::ToSql + Sync)])
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

- [ ] Add `tokio-postgres` dependency (if not already transitive)
- [ ] Create `src/auth/auth_query.rs` with `AuthQueryExecutor`
- [ ] `new()` — eager connection, clear error on failure
- [ ] `fetch_password()` — parameterized query, fail fast
- [ ] Add `AuthQueryError` to `src/errors.rs`
- [ ] Register module in `src/auth/mod.rs`
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
- [ ] Existing tests pass
