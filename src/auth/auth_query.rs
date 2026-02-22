//! Auth query executor for fetching credentials from PostgreSQL.
//!
//! Manages a small pool of persistent connections via an mpsc channel.
//! Connections are created eagerly at startup and recycled after each query.

use log::{debug, error, info, warn};
use tokio::sync::mpsc;
use tokio_postgres::{Client, NoTls};

use crate::config::AuthQueryConfig;
use crate::errors::Error;

/// Executor for running auth_query SELECT statements against PostgreSQL.
///
/// Uses an mpsc channel as a simple connection pool: `fetch_password()` takes
/// a Client from the channel, executes the query, and returns it back.
/// If all connections are busy, callers wait on the channel.
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
    /// All connections MUST succeed before accepting client traffic
    /// (prevents max_connections deadlock).
    pub async fn new(
        config: &AuthQueryConfig,
        pool_name: &str,
        server_host: &str,
        server_port: u16,
    ) -> Result<Self, Error> {
        let database = config
            .database
            .clone()
            .unwrap_or_else(|| pool_name.to_string());

        let pg_config = Self::build_pg_config(config, server_host, server_port, &database);

        let (tx, rx) = mpsc::channel(config.pool_size as usize);

        for i in 0..config.pool_size {
            info!(
                "[pool: {pool_name}] auth_query: opening executor connection {}/{} \
                 to {server_host}:{server_port}/{database} as '{}'",
                i + 1,
                config.pool_size,
                config.user
            );
            let client = Self::connect(
                &pg_config,
                i,
                pool_name,
                server_host,
                server_port,
                &database,
                &config.user,
            )
            .await?;
            tx.send(client).await.map_err(|_| {
                Error::AuthQueryConnectionError("failed to initialize executor pool".into())
            })?;
        }

        info!(
            "[pool: {pool_name}] auth_query executor ready: \
             {}@{server_host}:{server_port}/{database} (pool_size={})",
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
        pool_name: &str,
        server_host: &str,
        server_port: u16,
        database: &str,
        user: &str,
    ) -> Result<Client, Error> {
        let start = std::time::Instant::now();
        let (client, connection) = pg_config.connect(NoTls).await.map_err(|e| {
            error!(
                "[pool: {pool_name}] auth_query: executor connection {index} failed to \
                 {server_host}:{server_port}/{database} as '{user}': {e}"
            );
            Error::AuthQueryConnectionError(format!(
                "connection {index} to {server_host}:{server_port}/{database} as '{user}': {e}"
            ))
        })?;
        let elapsed = start.elapsed();

        info!(
            "[pool: {pool_name}] auth_query: executor connection {index} established \
             to {server_host}:{server_port}/{database} as '{user}' ({elapsed:.1?})"
        );

        let pool_name_owned = pool_name.to_string();
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!(
                    "[pool: {pool_name_owned}] auth_query executor connection {index} lost: {e}"
                );
            }
        });

        Ok(client)
    }

    /// Fetch password hash for a username from PostgreSQL.
    /// Returns `Some((username, password_hash))` or `None` if user not found.
    pub async fn fetch_password(&self, username: &str) -> Result<Option<(String, String)>, Error> {
        debug!(
            "[pool: {}] auth_query: fetching password for user '{username}'",
            self.pool_name
        );

        let client = {
            let mut rx = self.rx.lock().await;
            rx.recv().await.ok_or_else(|| {
                error!(
                    "[pool: {}] auth_query: executor pool closed, \
                     cannot fetch password for user '{username}'",
                    self.pool_name
                );
                Error::AuthQueryPoolClosed
            })?
        };

        let start = std::time::Instant::now();
        let result = self.execute_query(&client, username).await;
        let elapsed = start.elapsed();

        match &result {
            Ok(Some((user, _))) => {
                debug!(
                    "[pool: {}] auth_query: user '{user}' found ({elapsed:.1?})",
                    self.pool_name
                );
            }
            Ok(None) => {
                debug!(
                    "[pool: {}] auth_query: user '{username}' not found ({elapsed:.1?})",
                    self.pool_name
                );
            }
            Err(e) => {
                error!(
                    "[pool: {}] auth_query: query failed for user '{username}' \
                     ({elapsed:.1?}): {e}",
                    self.pool_name
                );
            }
        }

        // Return connection to pool, or reconnect if dead
        if result.is_ok() || !client.is_closed() {
            let _ = self.tx.send(client).await;
        } else {
            warn!(
                "[pool: {}] auth_query: executor connection dead after query failure, \
                 attempting reconnect",
                self.pool_name
            );
            self.try_reconnect().await;
        }

        result
    }

    async fn try_reconnect(&self) {
        let database = self
            .config
            .database
            .clone()
            .unwrap_or_else(|| self.pool_name.clone());
        let pg_config =
            Self::build_pg_config(&self.config, &self.server_host, self.server_port, &database);
        match Self::connect(
            &pg_config,
            0,
            &self.pool_name,
            &self.server_host,
            self.server_port,
            &database,
            &self.config.user,
        )
        .await
        {
            Ok(new_client) => {
                info!(
                    "[pool: {}] auth_query: executor reconnection successful",
                    self.pool_name
                );
                let _ = self.tx.send(new_client).await;
            }
            Err(e) => {
                error!(
                    "[pool: {}] auth_query: executor reconnection failed: {e} \
                     (pool shrinks by 1, will retry on next request)",
                    self.pool_name
                );
            }
        }
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
                Error::AuthQueryQueryError(format!(
                    "query execution failed for user '{username}': {e}"
                ))
            })?;

        match rows.len() {
            0 => Ok(None),
            1 => Self::extract_credentials(&rows[0], username),
            n => Err(Error::AuthQueryConfigError(format!(
                "query returned {n} rows for user '{username}', expected 0 or 1"
            ))),
        }
    }

    fn extract_credentials(
        row: &tokio_postgres::Row,
        username: &str,
    ) -> Result<Option<(String, String)>, Error> {
        let user: String = row.try_get(0).map_err(|e| {
            Error::AuthQueryConfigError(format!("failed to read username column: {e}"))
        })?;
        let password: Option<String> = row.try_get(1).map_err(|e| {
            Error::AuthQueryConfigError(format!("failed to read password column: {e}"))
        })?;
        match password {
            Some(p) if !p.is_empty() => Ok(Some((user, p))),
            _ => {
                warn!("auth_query: user '{username}' has NULL or empty password in pg_shadow");
                Ok(None)
            }
        }
    }
}
