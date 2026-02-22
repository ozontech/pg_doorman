# Step 2: AuthQueryExecutor

## Goal

Create the executor that connects to PostgreSQL and runs auth queries.
Uses `tokio-postgres` (already a dependency) with an mpsc channel as
a simple connection pool.

## Dependencies

- Step 1 (AuthQueryConfig struct)

## 2.1 Dependencies

`tokio-postgres` is already in `Cargo.toml` — no new dependencies needed.

## 2.2 AuthQueryExecutor struct

### File: `src/auth/auth_query.rs` (NEW)

**Connection pool via mpsc channel:**

Instead of `Semaphore` + `Vec<Mutex<Client>>` (which has race conditions
between permit acquisition and mutex locking), use a bounded mpsc channel
as a connection pool:

- `new()` creates N connections, sends them into the channel
- `fetch_password()` receives a Client from the channel, uses it, sends it back
- Natural backpressure: if all connections are busy, `.recv()` awaits
- No race conditions, no spurious "all connections busy" errors

```rust
use log::{error, info, warn};
use tokio::sync::mpsc;
use tokio_postgres::{Client, NoTls};

use crate::config::AuthQueryConfig;
use crate::errors::Error;

/// Executor for running auth_query SELECT statements against PostgreSQL.
///
/// Manages a small pool of persistent connections via an mpsc channel.
/// Connections are created eagerly at startup and recycled after each query.
pub struct AuthQueryExecutor {
    config: AuthQueryConfig,
    pool_name: String,
    server_host: String,
    server_port: u16,
    tx: mpsc::Sender<Client>,
    rx: tokio::sync::Mutex<mpsc::Receiver<Client>>,
}

impl AuthQueryExecutor {
    /// Create executor and establish connections eagerly.
    /// Called at pg_doorman startup. All connections MUST succeed before
    /// accepting client traffic (prevents max_connections deadlock).
    pub async fn new(
        config: &AuthQueryConfig,
        pool_name: &str,
        server_host: &str,
        server_port: u16,
    ) -> Result<Self, Error> {
        let database = config.database.clone()
            .unwrap_or_else(|| pool_name.to_string());

        let pg_config = Self::build_pg_config(config, server_host, server_port, &database);

        // Channel capacity = pool_size (bounded, no overallocation)
        let (tx, rx) = mpsc::channel(config.pool_size as usize);

        // Eagerly open all connections at startup
        for i in 0..config.pool_size {
            let client = Self::connect(&pg_config, i, server_host, server_port, &database, &config.user).await?;
            tx.send(client).await.map_err(|_| {
                Error::AuthQueryError("failed to initialize executor pool".into())
            })?;
        }

        info!(
            "auth_query executor ready: {}@{server_host}:{server_port}/{database} \
             (pool_size={})",
            config.user, config.pool_size
        );

        Ok(Self {
            config: config.clone(),
            pool_name: pool_name.to_string(),
            server_host: server_host.to_string(),
            server_port,
            tx,
            rx: tokio::sync::Mutex::new(rx),
        })
    }

    fn build_pg_config(
        config: &AuthQueryConfig,
        server_host: &str,
        server_port: u16,
        database: &str,
    ) -> tokio_postgres::Config {
        let mut pg_config = tokio_postgres::Config::new();
        pg_config.host(server_host);
        pg_config.port(server_port);
        pg_config.user(&config.user);
        if !config.password.is_empty() {
            pg_config.password(&config.password);
        }
        pg_config.dbname(database);
        pg_config.connect_timeout(std::time::Duration::from_secs(5));
        pg_config
    }

    async fn connect(
        pg_config: &tokio_postgres::Config,
        index: u32,
        server_host: &str,
        server_port: u16,
        database: &str,
        user: &str,
    ) -> Result<Client, Error> {
        let (client, connection) = pg_config.connect(NoTls).await
            .map_err(|e| Error::AuthQueryError(format!(
                "executor connection {index} failed to \
                 {server_host}:{server_port}/{database} as '{user}': {e}"
            )))?;

        // Spawn the connection task (handles async protocol messages).
        // When this task ends, the Client becomes unusable.
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("auth_query executor connection lost: {e}");
            }
        });

        Ok(client)
    }

    /// Fetch password hash for a username from PostgreSQL.
    /// Returns Some((username, password_hash)) or None if user not found.
    pub async fn fetch_password(
        &self,
        username: &str,
    ) -> Result<Option<(String, String)>, Error> {
        // Take a connection from the pool (waits if all busy)
        let client = {
            let mut rx = self.rx.lock().await;
            rx.recv().await.ok_or_else(|| {
                Error::AuthQueryError("executor pool closed".into())
            })?
        };

        // Execute query and always return connection to pool
        let result = self.execute_query(&client, username).await;

        // Check if connection is still alive before returning to pool
        if result.is_ok() || !client.is_closed() {
            let _ = self.tx.send(client).await;
        } else {
            // Connection is dead — try to reconnect
            warn!("auth_query executor: connection lost, reconnecting");
            let database = self.config.database.clone()
                .unwrap_or_else(|| self.pool_name.clone());
            let pg_config = Self::build_pg_config(
                &self.config, &self.server_host, self.server_port, &database,
            );
            match Self::connect(&pg_config, 0, &self.server_host, self.server_port, &database, &self.config.user).await {
                Ok(new_client) => { let _ = self.tx.send(new_client).await; }
                Err(e) => {
                    error!("auth_query executor: reconnection failed: {e}");
                    // Pool shrinks by 1 — will recover on next reconnect attempt
                }
            }
        }

        result
    }

    async fn execute_query(
        &self,
        client: &Client,
        username: &str,
    ) -> Result<Option<(String, String)>, Error> {
        let rows = client
            .query(
                &self.config.query,
                &[&username as &(dyn tokio_postgres::types::ToSql + Sync)],
            )
            .await
            .map_err(|e| {
                error!("auth_query execution failed for user '{username}': {e}");
                Error::AuthQueryError(format!("query execution failed: {e}"))
            })?;

        match rows.len() {
            0 => Ok(None),
            1 => Self::extract_credentials(&rows[0], username),
            n => {
                warn!("auth_query returned {n} rows for user '{username}', using first");
                Self::extract_credentials(&rows[0], username)
            }
        }
    }

    fn extract_credentials(
        row: &tokio_postgres::Row,
        username: &str,
    ) -> Result<Option<(String, String)>, Error> {
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
                Ok(None) // Treat as "user not found" — no auth possible
            }
        }
    }
}
```

### Key design decisions

1. **mpsc channel as pool**: `Sender<Client>` + `Receiver<Client>` bounded to `pool_size`.
   Take connection with `rx.recv()`, return with `tx.send()`. Natural backpressure —
   callers wait when all connections are busy. No race conditions.

2. **Reconnection on dead connection**: After query failure, check `client.is_closed()`.
   If dead — reconnect and return new client to pool. If reconnect fails — pool shrinks
   by 1 (self-healing on next attempt).

3. **Trust mode**: `build_pg_config()` only sets password if non-empty, supporting
   PostgreSQL trust auth for the executor user.

4. **`rx` behind `tokio::sync::Mutex`**: Only one caller can wait for a connection at a
   time. With pool_size=2 and auth requests coming in bursts, this serializes connection
   acquisition but not query execution (connection is held outside the lock).

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

## 2.4 TLS

Executor uses `NoTls` — same as the main pooler. pg_doorman does NOT support
TLS for backend (pg_doorman → PostgreSQL) connections: `server_tls` config field
exists and an SSL request is sent, but the TLS handshake is not implemented
(`src/server/stream.rs` returns error on 'S' response). When backend TLS is
implemented for the main pooler, it should be propagated to executor connections
as well.

## Checklist

- [ ] Create `src/auth/auth_query.rs` with `AuthQueryExecutor`
- [ ] `new()` — eager connections via mpsc channel
- [ ] `fetch_password()` — take from channel, query, return to channel
- [ ] Dead connection detection + reconnection
- [ ] Trust mode support (empty password)
- [ ] Add `AuthQueryError` to `src/errors.rs`
- [ ] Register module in `src/auth/mod.rs`
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
- [ ] Existing tests pass
