//! Auth query executor and cache for fetching credentials from PostgreSQL.
//!
//! Two main components:
//! - `AuthQueryExecutor`: manages a small pool of persistent connections via
//!   an mpsc channel and executes parameterized SELECT queries.
//! - `AuthQueryCache`: per-pool credential cache with double-checked locking,
//!   TTL-based expiration, negative caching, and rate-limited re-fetch.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use log::{debug, error, info, warn};

use crate::utils::format_elapsed;
use tokio::sync::mpsc;
use tokio::sync::Mutex as TokioMutex;
use tokio_postgres::{Client, NoTls};

use crate::config::{AuthQueryConfig, Duration};
use crate::errors::Error;
use crate::stats::auth_query::AuthQueryStats;

/// Maximum username length accepted by the cache.
/// PostgreSQL limits role names to NAMEDATALEN - 1 = 63 bytes.
/// Usernames exceeding this are rejected without caching to prevent
/// memory exhaustion from very long usernames.
const MAX_USERNAME_LEN: usize = 63;

// ---------------------------------------------------------------------------
// PasswordFetcher trait (allows mocking AuthQueryExecutor in unit tests)
// ---------------------------------------------------------------------------

/// Password hash plus the per-user startup_parameters map returned by
/// auth_query. The map is empty when the optional column is absent, NULL,
/// empty, or fully rejected by validation.
pub type Credentials = (String, std::collections::HashMap<String, String>);

/// Trait for fetching credentials from PostgreSQL.
/// `AuthQueryExecutor` implements this; tests and benchmarks can substitute a mock.
///
/// `fetch` returns the password hash. `fetch_credentials` also returns the
/// optional per-user startup parameter map. Fetchers that do not support that
/// column use the default empty map.
pub trait PasswordFetcher: Send + Sync {
    fn fetch<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = Result<Option<String>, Error>> + Send + 'a;

    fn fetch_credentials<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = Result<Option<Credentials>, Error>> + Send + 'a {
        async move {
            Ok(self
                .fetch(username)
                .await?
                .map(|p| (p, std::collections::HashMap::new())))
        }
    }
}

impl PasswordFetcher for AuthQueryExecutor {
    fn fetch<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = Result<Option<String>, Error>> + Send + 'a {
        self.fetch_password(username)
    }

    fn fetch_credentials<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = Result<Option<Credentials>, Error>> + Send + 'a {
        AuthQueryExecutor::fetch_credentials(self, username)
    }
}

// ---------------------------------------------------------------------------
// AuthQueryExecutor
// ---------------------------------------------------------------------------

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

        let (tx, rx) = mpsc::channel(config.workers as usize);

        for i in 0..config.workers {
            info!(
                "[pool: {pool_name}] auth_query: opening executor connection {}/{} \
                 to {server_host}:{server_port}/{database} as '{}'",
                i + 1,
                config.workers,
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
             {}@{server_host}:{server_port}/{database} (workers={})",
            config.user, config.workers
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
        let elapsed = format_elapsed(start.elapsed());

        info!(
            "[pool: {pool_name}] auth_query: executor connection {index} established \
             to {server_host}:{server_port}/{database} as '{user}' ({elapsed})"
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

    /// Fetch credentials (password hash plus the optional per-user
    /// startup_parameters map) for a username from PostgreSQL.
    /// Returns `Some((password_hash, params))` or `None` if user not found.
    pub async fn fetch_credentials(&self, username: &str) -> Result<Option<Credentials>, Error> {
        debug!(
            "[{username}@{}] auth_query: fetching credentials",
            self.pool_name
        );

        let client = {
            let mut rx = self.rx.lock().await;
            rx.recv().await.ok_or_else(|| {
                error!(
                    "[{username}@{}] auth_query: executor pool closed, cannot fetch credentials",
                    self.pool_name
                );
                Error::AuthQueryPoolClosed
            })?
        };

        let start = std::time::Instant::now();
        let result = self.execute_query(&client, username).await;
        let elapsed = format_elapsed(start.elapsed());

        match &result {
            Ok(Some(_)) => {
                debug!(
                    "[{username}@{}] auth_query: password found ({elapsed})",
                    self.pool_name
                );
            }
            Ok(None) => {
                debug!(
                    "[{username}@{}] auth_query: user not found ({elapsed})",
                    self.pool_name
                );
            }
            Err(e) => {
                error!(
                    "[{username}@{}] auth_query: query failed ({elapsed}): {e}",
                    self.pool_name
                );
            }
        }

        // Return connection to pool, or reconnect if dead
        if result.is_ok() || !client.is_closed() {
            let _ = self.tx.send(client).await;
        } else {
            warn!(
                "[{username}@{}] auth_query: executor connection dead after query failure, \
                 attempting reconnect",
                self.pool_name
            );
            self.try_reconnect().await;
        }

        result
    }

    /// Backwards-compatible password-only accessor. Discards any per-user
    /// startup_parameters returned alongside the password.
    pub async fn fetch_password(&self, username: &str) -> Result<Option<String>, Error> {
        Ok(self.fetch_credentials(username).await?.map(|(p, _)| p))
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
    ) -> Result<Option<Credentials>, Error> {
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
            1 => {
                let row = &rows[0];
                let pw_opt = Self::extract_password(row, username, &self.pool_name)?;
                let Some(pw) = pw_opt else {
                    return Ok(None);
                };
                let params = Self::extract_startup_parameters(row, username, &self.pool_name);
                Ok(Some((pw, params)))
            }
            n => Err(Error::AuthQueryConfigError(format!(
                "query returned {n} rows for user '{username}', expected 0 or 1"
            ))),
        }
    }

    /// Extract password hash from query result row.
    ///
    /// Column lookup priority:
    /// 1. Column named `passwd` (matches `pg_shadow.passwd`)
    /// 2. Column named `password`
    /// 3. If the query returns exactly one column, use it regardless of name
    fn extract_password(
        row: &tokio_postgres::Row,
        username: &str,
        pool_name: &str,
    ) -> Result<Option<String>, Error> {
        let columns = row.columns();
        let password: Option<String> = if let Ok(p) = row.try_get::<_, Option<String>>("passwd") {
            p
        } else if let Ok(p) = row.try_get::<_, Option<String>>("password") {
            p
        } else if columns.len() == 1 {
            row.try_get::<_, Option<String>>(0).map_err(|e| {
                Error::AuthQueryConfigError(format!(
                    "failed to read password from single-column result: {e}"
                ))
            })?
        } else {
            let col_names: Vec<&str> = columns.iter().map(|c| c.name()).collect();
            return Err(Error::AuthQueryConfigError(format!(
                "cannot find password column for user '{username}': \
                 expected column named 'passwd' or 'password', or a single-column result; \
                 got columns: {col_names:?}"
            )));
        };
        match password {
            Some(p) if !p.is_empty() => Ok(Some(p)),
            _ => {
                warn!("[{username}@{pool_name}] auth_query: password is NULL or empty");
                Ok(None)
            }
        }
    }

    /// Read the optional `startup_parameters` text column from the auth_query
    /// row and parse it as a JSON object. A missing column yields an empty
    /// map; a present column whose type does not coerce to `Option<String>`
    /// logs a warning and yields an empty map. Actual JSON parsing and
    /// per-entry validation happen in `parse_startup_parameters_text`.
    fn extract_startup_parameters(
        row: &tokio_postgres::Row,
        username: &str,
        pool_name: &str,
    ) -> std::collections::HashMap<String, String> {
        let column = row
            .columns()
            .iter()
            .find(|c| c.name() == "startup_parameters");
        let Some(column) = column else {
            return std::collections::HashMap::new();
        };
        let raw: Option<String> = match row.try_get::<_, Option<String>>("startup_parameters") {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "[{username}@{pool_name}] auth_query startup_parameters column has type \
                     `{ty}` but pg_doorman reads it as `text`: {e}. If the SELECT returns \
                     json or jsonb, add `::text` (for example: \
                     `jsonb_build_object(...)::text AS startup_parameters`); per-user \
                     parameters are ignored for this row.",
                    ty = column.type_().name()
                );
                crate::web::metrics::STARTUP_PARAMETERS_DROPPED_TOTAL
                    .with_label_values(&[pool_name, "auth_query_bad_type"])
                    .inc();
                return std::collections::HashMap::new();
            }
        };
        Self::parse_startup_parameters_text(raw.as_deref(), username, pool_name)
    }

    /// Parse the optional `startup_parameters` JSON object returned by
    /// auth_query. Valid string entries become per-user GUCs. Invalid keys,
    /// non-string values, malformed JSON, and non-object JSON are logged and
    /// ignored; authentication still continues.
    fn parse_startup_parameters_text(
        text: Option<&str>,
        username: &str,
        pool_name: &str,
    ) -> std::collections::HashMap<String, String> {
        let Some(text) = text else {
            return std::collections::HashMap::new();
        };
        if text.is_empty() {
            return std::collections::HashMap::new();
        }
        // Reject oversize input before serde_json allocates the Value tree.
        // A single auth_query row above the operator budget cannot produce a
        // sendable startup map.
        let max_bytes = crate::config::startup_parameters::MAX_OPERATOR_BUDGET;
        if text.len() > max_bytes {
            warn!(
                "[{username}@{pool_name}] auth_query startup_parameters: raw column is {} bytes, \
                 exceeding operator budget {max_bytes}; parameters ignored",
                text.len()
            );
            crate::web::metrics::STARTUP_PARAMETERS_DROPPED_TOTAL
                .with_label_values(&[pool_name, "auth_query_oversize"])
                .inc();
            return std::collections::HashMap::new();
        }
        let parsed: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "[{username}@{pool_name}] auth_query startup_parameters: JSON parse failed: \
                     {e}; parameters ignored"
                );
                crate::web::metrics::STARTUP_PARAMETERS_DROPPED_TOTAL
                    .with_label_values(&[pool_name, "auth_query_invalid_json"])
                    .inc();
                return std::collections::HashMap::new();
            }
        };
        let serde_json::Value::Object(obj) = parsed else {
            warn!(
                "[{username}@{pool_name}] auth_query startup_parameters: top-level value is not a \
                 JSON object; ignored"
            );
            crate::web::metrics::STARTUP_PARAMETERS_DROPPED_TOTAL
                .with_label_values(&[pool_name, "auth_query_invalid_shape"])
                .inc();
            return std::collections::HashMap::new();
        };
        let mut out = std::collections::HashMap::new();
        let scope = format!("auth_query.startup_parameters[user={username}]");
        for (k, v) in obj {
            match v {
                serde_json::Value::String(s) => {
                    let mut probe = std::collections::BTreeMap::new();
                    probe.insert(k.clone(), s.clone());
                    if let Err(e) = crate::config::startup_parameters::validate(&probe, &scope) {
                        warn!("[{pool_name}] {e}");
                        crate::web::metrics::STARTUP_PARAMETERS_DROPPED_TOTAL
                            .with_label_values(&[pool_name, "auth_query_invalid_entry"])
                            .inc();
                        continue;
                    }
                    out.insert(k, s);
                }
                other => {
                    let kind = match other {
                        serde_json::Value::Null => "null",
                        serde_json::Value::Bool(_) => "boolean",
                        serde_json::Value::Number(_) => "number",
                        serde_json::Value::Array(_) => "array",
                        serde_json::Value::Object(_) => "object",
                        serde_json::Value::String(_) => unreachable!(),
                    };
                    warn!(
                        "[{username}@{pool_name}] auth_query startup_parameters: value for '{k}' \
                         is {kind}, not string; ignored"
                    );
                    crate::web::metrics::STARTUP_PARAMETERS_DROPPED_TOTAL
                        .with_label_values(&[pool_name, "auth_query_invalid_entry"])
                        .inc();
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// CacheEntry
// ---------------------------------------------------------------------------

/// Single cache entry for a username's credentials.
#[derive(Clone, Debug)]
pub struct CacheEntry {
    /// Password hash from pg_shadow ("md5..." or "SCRAM-SHA-256$...")
    pub password_hash: String,
    /// When this entry was fetched from PG
    pub fetched_at: Instant,
    /// True if user was NOT found in pg_shadow
    pub is_negative: bool,
    /// When was the last re-fetch attempted for this user (rate limiting)
    pub last_refetch_at: Option<Instant>,
    /// SCRAM ClientKey extracted from client's proof (Step 5).
    /// Stored here so pool connections created later can use it
    /// for SCRAM passthrough to backend PG (Step 6).
    /// None for MD5 users or before first SCRAM auth.
    pub client_key: Option<Vec<u8>>,
    /// Per-user startup parameters returned by the optional auth_query
    /// `startup_parameters` JSON column. Empty when the column is absent,
    /// empty/NULL, or filtered out in dedicated auth_query mode.
    pub startup_parameters: std::collections::HashMap<String, String>,
}

impl CacheEntry {
    fn positive(password_hash: String) -> Self {
        Self {
            password_hash,
            fetched_at: Instant::now(),
            is_negative: false,
            last_refetch_at: None,
            client_key: None,
            startup_parameters: std::collections::HashMap::new(),
        }
    }

    fn negative() -> Self {
        Self {
            password_hash: String::new(),
            fetched_at: Instant::now(),
            is_negative: true,
            last_refetch_at: None,
            client_key: None,
            startup_parameters: std::collections::HashMap::new(),
        }
    }

    fn is_expired(&self, cache_ttl: &Duration, cache_failure_ttl: &Duration) -> bool {
        let ttl_ms = if self.is_negative {
            cache_failure_ttl.as_millis()
        } else {
            cache_ttl.as_millis()
        };
        self.fetched_at.elapsed().as_millis() as u64 >= ttl_ms
    }
}

// ---------------------------------------------------------------------------
// AuthQueryCache
// ---------------------------------------------------------------------------

/// Per-pool auth query cache with double-checked locking.
///
/// Caches credentials fetched by `AuthQueryExecutor` to avoid hitting PG
/// on every client authentication. Supports:
/// - Positive caching (user found) with `cache_ttl`
/// - Negative caching (user not found) with `cache_failure_ttl`
/// - Per-username locks for request coalescing (double-checked locking)
/// - Rate-limited re-fetch after auth failure (`min_interval`)
///
/// Generic over the fetcher: defaults to `AuthQueryExecutor` in production,
/// tests substitute a mock.
pub struct AuthQueryCache<F = AuthQueryExecutor> {
    /// Pool name for log context.
    pool_name: String,
    /// Cached credentials keyed by username.
    entries: DashMap<String, CacheEntry>,
    /// Per-username locks for request coalescing.
    /// First request acquires lock + fetches; others wait + get cache hit.
    locks: DashMap<String, Arc<TokioMutex<()>>>,
    /// Fetcher for cache miss to PG.
    executor: Arc<F>,
    /// TTL for positive cache entries (user found).
    cache_ttl: Duration,
    /// TTL for negative cache entries (user not found).
    cache_failure_ttl: Duration,
    /// Minimum interval between re-fetches (rate limiting).
    min_interval: Duration,
    /// Optional stats for observability (None in unit tests).
    stats: Option<Arc<AuthQueryStats>>,
    /// True when auth_query runs in dedicated mode (server_user is set).
    /// In that mode every backend connection shares a single backend
    /// identity, so per-user startup_parameters cannot be honored.
    is_dedicated: bool,
    /// Usernames already warned about dropped per-user startup_parameters
    /// in dedicated mode. Ensures the warning fires at most once per
    /// (pool, user) until the cache is cleared by a config reload.
    dedicated_warnings: DashMap<String, ()>,
}

impl<F: PasswordFetcher> AuthQueryCache<F> {
    pub fn new(
        pool_name: String,
        executor: Arc<F>,
        config: &AuthQueryConfig,
        stats: Option<Arc<AuthQueryStats>>,
    ) -> Self {
        Self {
            pool_name,
            entries: DashMap::new(),
            locks: DashMap::new(),
            executor,
            cache_ttl: config.cache_ttl,
            cache_failure_ttl: config.cache_failure_ttl,
            min_interval: config.min_interval,
            stats,
            is_dedicated: config.is_dedicated_mode(),
            dedicated_warnings: DashMap::new(),
        }
    }

    /// In dedicated auth_query mode (`server_user` set) every backend
    /// connection shares a single identity, so per-user startup_parameters
    /// cannot be honored: pg_doorman has no per-user backend on which to
    /// apply them. Drop the parsed map before it reaches downstream code
    /// and warn once per (pool, username) so the operator notices.
    fn dedicated_mode_filter(&self, entry: &mut CacheEntry, username: &str) {
        if !self.is_dedicated || entry.startup_parameters.is_empty() {
            return;
        }
        // Every dropped entry contributes to the metric so operators can
        // see the volume of per-user GUCs lost to dedicated mode without
        // log scraping. The warn-log itself stays once per (pool, user)
        // to keep the log readable.
        crate::web::metrics::STARTUP_PARAMETERS_DROPPED_TOTAL
            .with_label_values(&[self.pool_name.as_str(), "dedicated_mode"])
            .inc_by(entry.startup_parameters.len() as u64);
        if self
            .dedicated_warnings
            .insert(username.to_string(), ())
            .is_none()
        {
            warn!(
                "[{username}@{pool}] per-user startup_parameters ignored in dedicated \
                 auth_query mode; use pool-level startup_parameters instead",
                pool = self.pool_name
            );
        }
        entry.startup_parameters.clear();
    }

    /// When a fresh auth_query fetch produces a per-user
    /// `startup_parameters` map that differs from the snapshot frozen
    /// into the live dynamic pool at creation time, drop the pool so
    /// the next client connection rebuilds against the new overlay.
    /// Without this, an operator-side change to the row (`UPDATE
    /// pgbouncer.users SET startup_parameters = ...`) only takes effect
    /// for new dynamic-pool spawns, not for existing pools. Dedicated
    /// mode and the dedicated-mode warning path land here with an empty
    /// map; that compares equal to the empty-overlay hash that
    /// dedicated pools store, so nothing is dropped on that path.
    fn drop_dynamic_pool_if_overlay_drifted(
        &self,
        username: &str,
        new_overlay: &std::collections::HashMap<String, String>,
    ) {
        let identifier = crate::pool::PoolIdentifier::new(&self.pool_name, username);
        if !crate::pool::is_dynamic_pool(&identifier) {
            return;
        }
        let new_hash = crate::pool::per_user_overlay_hash(new_overlay.iter());
        let live_hash = crate::pool::POOLS
            .load()
            .get(&identifier)
            .map(|p| p.per_user_startup_overlay_hash);
        match live_hash {
            Some(h) if h != new_hash => {
                if crate::pool::drop_dynamic_pool(&identifier) {
                    info!(
                        "[{username}@{}] auth_query overlay drift on refetch — dynamic pool dropped, next connect will rebuild",
                        self.pool_name
                    );
                }
            }
            _ => {}
        }
    }

    /// Increment a stats counter if stats are enabled.
    fn inc(&self, counter: fn(&AuthQueryStats) -> &AtomicU64) {
        if let Some(ref stats) = self.stats {
            counter(stats).fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get password hash for username. Uses cache with double-checked locking.
    ///
    /// Returns:
    /// - `Ok(Some(entry))` — user found (positive cache or fresh fetch)
    /// - `Ok(None)` — user not found (negative cache or fresh fetch returned 0 rows)
    /// - `Err` — executor error (PG down, SQL error, etc.)
    pub async fn get_or_fetch(&self, username: &str) -> Result<Option<CacheEntry>, Error> {
        if username.len() > MAX_USERNAME_LEN {
            warn!(
                "[{username}@{}] auth_query cache: rejecting username (len={}, max={MAX_USERNAME_LEN})",
                self.pool_name,
                username.len()
            );
            return Ok(None);
        }

        // Fast path: check cache without lock
        if let Some(entry) = self.entries.get(username) {
            if !entry.is_expired(&self.cache_ttl, &self.cache_failure_ttl) {
                self.inc(|s| &s.cache_hits);
                return if entry.is_negative {
                    Ok(None)
                } else {
                    Ok(Some(entry.clone()))
                };
            }
        }

        // Slow path: acquire per-username lock
        let lock = self
            .locks
            .entry(username.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();

        let _guard = lock.lock().await;

        // Double-check after acquiring lock
        if let Some(entry) = self.entries.get(username) {
            if !entry.is_expired(&self.cache_ttl, &self.cache_failure_ttl) {
                self.inc(|s| &s.cache_hits);
                return if entry.is_negative {
                    Ok(None)
                } else {
                    Ok(Some(entry.clone()))
                };
            }
        }

        // Cache miss: fetch credentials from PG.
        self.inc(|s| &s.executor_queries);
        match self.executor.fetch_credentials(username).await {
            Ok(Some((password_hash, startup_params))) => {
                self.inc(|s| &s.cache_misses);
                let mut entry = CacheEntry::positive(password_hash);
                entry.startup_parameters = startup_params;
                self.dedicated_mode_filter(&mut entry, username);
                // Publish the fresh entry first so any concurrent
                // create_dynamic_pool peeks the new overlay, then drop the
                // pool whose snapshot drifted. Reversing the order would
                // open a window where the drop runs against the live pool
                // while the cache still holds the old map, and a racing
                // create_dynamic_pool would rebuild against that stale
                // map and immediately drift again.
                self.entries.insert(username.to_string(), entry.clone());
                self.drop_dynamic_pool_if_overlay_drifted(username, &entry.startup_parameters);
                Ok(Some(entry))
            }
            Ok(None) => {
                self.inc(|s| &s.cache_misses);
                let entry = CacheEntry::negative();
                self.entries.insert(username.to_string(), entry);
                Ok(None)
            }
            Err(err) => {
                self.inc(|s| &s.executor_errors);
                Err(err)
            }
        }
    }

    /// Invalidate cache entry for a username.
    /// Called on auth failure to trigger re-fetch on next attempt.
    pub fn invalidate(&self, username: &str) {
        if self.entries.remove(username).is_some() {
            info!(
                "[{username}@{}] auth_query cache: invalidated",
                self.pool_name
            );
        }
    }

    /// Attempt re-fetch after auth failure (password may have changed).
    /// Returns `Ok(Some(entry))` if re-fetched, `Ok(None)` if rate-limited or user gone.
    ///
    /// Rate limiting: won't re-fetch if last re-fetch was < `min_interval` ago.
    ///
    /// Uses the same per-username lock as `get_or_fetch()` to prevent concurrent
    /// refetches for the same user.
    pub async fn refetch_on_failure(&self, username: &str) -> Result<Option<CacheEntry>, Error> {
        // Acquire per-username lock (same lock as get_or_fetch)
        let lock = self
            .locks
            .entry(username.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();

        let _guard = lock.lock().await;

        // Check rate limit (under lock to avoid TOCTOU)
        if let Some(entry) = self.entries.get(username) {
            if let Some(last) = entry.last_refetch_at {
                if last.elapsed() < self.min_interval.as_std() {
                    self.inc(|s| &s.cache_rate_limited);
                    warn!(
                        "[{username}@{}] auth_query cache: refetch rate-limited ({} since last)",
                        self.pool_name,
                        format_elapsed(last.elapsed())
                    );
                    return Ok(None); // Rate limited
                }
            }
        }

        // Fetch fresh from PG.
        self.inc(|s| &s.executor_queries);
        self.inc(|s| &s.cache_refetches);
        match self.executor.fetch_credentials(username).await {
            Ok(Some((password_hash, startup_params))) => {
                let mut entry = CacheEntry::positive(password_hash);
                entry.startup_parameters = startup_params;
                entry.last_refetch_at = Some(Instant::now());
                self.dedicated_mode_filter(&mut entry, username);
                // Insert before drop — see comment in get_or_fetch.
                self.entries.insert(username.to_string(), entry.clone());
                self.drop_dynamic_pool_if_overlay_drifted(username, &entry.startup_parameters);
                Ok(Some(entry))
            }
            Ok(None) => {
                let mut entry = CacheEntry::negative();
                entry.last_refetch_at = Some(Instant::now());
                self.entries.insert(username.to_string(), entry);
                Ok(None)
            }
            Err(err) => {
                self.inc(|s| &s.executor_errors);
                Err(err)
            }
        }
    }

    /// Clear all entries (called on RELOAD when auth_query config changes).
    /// Also resets dedicated-mode warning suppression after reload.
    pub fn clear(&self) {
        self.entries.clear();
        self.locks.clear();
        self.dedicated_warnings.clear();
    }

    /// Store ClientKey for a cached user (called after successful SCRAM auth).
    pub fn set_client_key(&self, username: &str, client_key: Vec<u8>) {
        if let Some(mut entry) = self.entries.get_mut(username) {
            entry.client_key = Some(client_key);
        }
    }

    /// Get stored ClientKey for a cached user (for SCRAM passthrough).
    pub fn get_client_key(&self, username: &str) -> Option<Vec<u8>> {
        self.entries
            .get(username)
            .and_then(|e| e.client_key.clone())
    }

    /// Synchronous lookup of the cached per-user startup_parameters map.
    /// Returns `None` when there is no positive, unexpired cache entry. This
    /// never queries PostgreSQL or initializes the executor.
    ///
    /// The TTL check prevents replenishment and anticipation from using stale
    /// per-user GUCs after the auth_query row should have expired.
    pub fn peek_startup_parameters<R>(
        &self,
        username: &str,
        f: impl FnOnce(&std::collections::HashMap<String, String>) -> R,
    ) -> Option<R> {
        // Closure-based to avoid cloning the cached HashMap on every
        // backend spawn. The DashMap shard read-lock is held only for the
        // duration of `f`; consumers merge the overlay directly into their
        // owned destination map instead of through an intermediate clone.
        let entry = self.entries.get(username)?;
        if entry.is_negative {
            return None;
        }
        if entry.is_expired(&self.cache_ttl, &self.cache_failure_ttl) {
            return None;
        }
        Some(f(&entry.startup_parameters))
    }

    /// Number of cached entries (for metrics/admin).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock fetcher for unit tests.
    /// Pre-configure responses; fetch calls are counted.
    struct MockFetcher {
        responses: DashMap<String, Option<String>>,
        /// Optional per-user startup_parameters map. Surfaced via
        /// `fetch_credentials` so cache-side wiring can be exercised
        /// without standing up a real PG.
        params: DashMap<String, std::collections::HashMap<String, String>>,
        fetch_count: AtomicUsize,
        /// Optional delay to simulate slow PG queries (for concurrency tests).
        delay: std::time::Duration,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                responses: DashMap::new(),
                params: DashMap::new(),
                fetch_count: AtomicUsize::new(0),
                delay: std::time::Duration::ZERO,
            }
        }

        fn with_delay(delay: std::time::Duration) -> Self {
            Self {
                responses: DashMap::new(),
                params: DashMap::new(),
                fetch_count: AtomicUsize::new(0),
                delay,
            }
        }

        fn add_user(&self, username: &str, password_hash: &str) {
            self.responses
                .insert(username.to_string(), Some(password_hash.to_string()));
        }

        fn add_user_with_params(
            &self,
            username: &str,
            password_hash: &str,
            params: &[(&str, &str)],
        ) {
            self.responses
                .insert(username.to_string(), Some(password_hash.to_string()));
            let map: std::collections::HashMap<String, String> = params
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            self.params.insert(username.to_string(), map);
        }

        fn fetch_count(&self) -> usize {
            self.fetch_count.load(Ordering::SeqCst)
        }
    }

    impl PasswordFetcher for MockFetcher {
        fn fetch<'a>(
            &'a self,
            username: &'a str,
        ) -> impl Future<Output = Result<Option<String>, Error>> + Send + 'a {
            self.fetch_count.fetch_add(1, Ordering::SeqCst);
            let result = self
                .responses
                .get(username)
                .map(|r| r.clone())
                .unwrap_or(None);
            let delay = self.delay;
            async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                Ok(result)
            }
        }

        fn fetch_credentials<'a>(
            &'a self,
            username: &'a str,
        ) -> impl Future<Output = Result<Option<Credentials>, Error>> + Send + 'a {
            self.fetch_count.fetch_add(1, Ordering::SeqCst);
            let pw = self
                .responses
                .get(username)
                .map(|r| r.clone())
                .unwrap_or(None);
            let params = self
                .params
                .get(username)
                .map(|r| r.clone())
                .unwrap_or_default();
            let delay = self.delay;
            async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                Ok(pw.map(|p| (p, params)))
            }
        }
    }

    fn test_config() -> AuthQueryConfig {
        AuthQueryConfig {
            query: String::new(),
            user: String::new(),
            password: String::new(),
            database: None,
            workers: 1,
            server_user: None,
            server_password: None,
            pool_size: 40,
            min_pool_size: 0,
            cache_ttl: Duration::from_hours(1),
            cache_failure_ttl: Duration::from_secs(30),
            min_interval: Duration::from_secs(1),
        }
    }

    fn make_cache(
        fetcher: Arc<MockFetcher>,
        config: &AuthQueryConfig,
    ) -> AuthQueryCache<MockFetcher> {
        AuthQueryCache::new("test_pool".to_string(), fetcher, config, None)
    }

    // -- test_cache_hit: second get_or_fetch returns cached, no extra fetch --

    #[tokio::test]
    async fn test_cache_hit() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("alice", "md5abc123");
        let cache = make_cache(fetcher.clone(), &test_config());

        // First call: cache miss → fetches from PG
        let entry = cache.get_or_fetch("alice").await.unwrap().unwrap();
        assert_eq!(entry.password_hash, "md5abc123");
        assert!(!entry.is_negative);
        assert_eq!(fetcher.fetch_count(), 1);

        // Second call: cache hit → no extra fetch
        let entry = cache.get_or_fetch("alice").await.unwrap().unwrap();
        assert_eq!(entry.password_hash, "md5abc123");
        assert_eq!(fetcher.fetch_count(), 1);
    }

    // -- test_cache_miss_fetches: empty cache triggers a fetch --

    #[tokio::test]
    async fn test_cache_miss_fetches() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("bob", "SCRAM-SHA-256$iter:salt$stored:server");
        let cache = make_cache(fetcher.clone(), &test_config());

        assert_eq!(fetcher.fetch_count(), 0);
        let entry = cache.get_or_fetch("bob").await.unwrap().unwrap();
        assert_eq!(entry.password_hash, "SCRAM-SHA-256$iter:salt$stored:server");
        assert_eq!(fetcher.fetch_count(), 1);
    }

    // -- test_cache_ttl_expiration: expired entry triggers re-fetch --

    #[tokio::test]
    async fn test_cache_ttl_expiration() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("alice", "md5abc123");
        let mut config = test_config();
        config.cache_ttl = Duration::from_millis(50);

        let cache = make_cache(fetcher.clone(), &config);

        cache.get_or_fetch("alice").await.unwrap();
        assert_eq!(fetcher.fetch_count(), 1);

        // Wait for TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        cache.get_or_fetch("alice").await.unwrap();
        assert_eq!(fetcher.fetch_count(), 2);
    }

    // -- test_negative_cache: user-not-found is cached with cache_failure_ttl --

    #[tokio::test]
    async fn test_negative_cache() {
        let fetcher = Arc::new(MockFetcher::new());
        // "unknown" not added → fetch returns None
        let mut config = test_config();
        config.cache_failure_ttl = Duration::from_millis(50);

        let cache = make_cache(fetcher.clone(), &config);

        // First call: fetch returns None, cached as negative
        assert!(cache.get_or_fetch("unknown").await.unwrap().is_none());
        assert_eq!(fetcher.fetch_count(), 1);

        // Second call: negative cache hit
        assert!(cache.get_or_fetch("unknown").await.unwrap().is_none());
        assert_eq!(fetcher.fetch_count(), 1);

        // Wait for failure TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Should re-fetch
        assert!(cache.get_or_fetch("unknown").await.unwrap().is_none());
        assert_eq!(fetcher.fetch_count(), 2);
    }

    // -- test_invalidate: removes entry, next fetch goes to PG --

    #[tokio::test]
    async fn test_invalidate() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("alice", "md5abc123");
        let cache = make_cache(fetcher.clone(), &test_config());

        cache.get_or_fetch("alice").await.unwrap();
        assert_eq!(fetcher.fetch_count(), 1);

        cache.invalidate("alice");

        cache.get_or_fetch("alice").await.unwrap();
        assert_eq!(fetcher.fetch_count(), 2);
    }

    // -- test_rate_limiting: refetch_on_failure respects min_interval --

    #[tokio::test]
    async fn test_rate_limiting() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("alice", "md5abc123");
        let mut config = test_config();
        config.min_interval = Duration::from_secs(10);

        let cache = make_cache(fetcher.clone(), &config);

        // First refetch: no previous refetch → succeeds
        let result = cache.refetch_on_failure("alice").await.unwrap();
        assert!(result.is_some());
        assert_eq!(fetcher.fetch_count(), 1);

        // Second refetch immediately: rate-limited
        let result = cache.refetch_on_failure("alice").await.unwrap();
        assert!(result.is_none());
        assert_eq!(fetcher.fetch_count(), 1); // No additional fetch
    }

    // -- test_double_checked_locking: concurrent requests → single fetch --

    #[tokio::test]
    async fn test_double_checked_locking() {
        let fetcher = Arc::new(MockFetcher::with_delay(std::time::Duration::from_millis(
            100,
        )));
        fetcher.add_user("alice", "md5abc123");
        let cache = Arc::new(make_cache(fetcher.clone(), &test_config()));

        // Spawn concurrent requests for the same user
        let mut handles = Vec::new();
        for _ in 0..10 {
            let cache = cache.clone();
            handles.push(tokio::spawn(
                async move { cache.get_or_fetch("alice").await },
            ));
        }

        for handle in handles {
            let result = handle.await.unwrap().unwrap().unwrap();
            assert_eq!(result.password_hash, "md5abc123");
        }

        // Double-checked locking: only one fetch despite 10 concurrent requests
        assert_eq!(fetcher.fetch_count(), 1);
    }

    // -- test_long_username_rejected: >63 chars → None without fetch or caching --

    #[tokio::test]
    async fn test_long_username_rejected() {
        let fetcher = Arc::new(MockFetcher::new());
        let cache = make_cache(fetcher.clone(), &test_config());

        let long_name = "a".repeat(MAX_USERNAME_LEN + 1);
        let result = cache.get_or_fetch(&long_name).await.unwrap();
        assert!(result.is_none());
        assert_eq!(fetcher.fetch_count(), 0); // No fetch attempted
        assert_eq!(cache.len(), 0); // Not cached
    }

    // -- test_clear: removes all entries and locks --

    #[tokio::test]
    async fn test_clear() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("alice", "md5abc123");
        fetcher.add_user("bob", "md5def456");
        let cache = make_cache(fetcher.clone(), &test_config());

        cache.get_or_fetch("alice").await.unwrap();
        cache.get_or_fetch("bob").await.unwrap();
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    // -- test_set_client_key: stores SCRAM ClientKey on existing entry --

    #[tokio::test]
    async fn test_set_client_key() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("alice", "SCRAM-SHA-256$iter:salt$stored:server");
        let cache = make_cache(fetcher.clone(), &test_config());

        let entry = cache.get_or_fetch("alice").await.unwrap().unwrap();
        assert!(entry.client_key.is_none());

        let key = vec![1, 2, 3, 4];
        cache.set_client_key("alice", key.clone());

        let entry = cache.get_or_fetch("alice").await.unwrap().unwrap();
        assert_eq!(entry.client_key, Some(key));
    }

    // -- test_stats_counters: verifies stats are incremented correctly --

    #[tokio::test]
    async fn test_stats_counters() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user("alice", "md5abc123");
        let stats = Arc::new(AuthQueryStats::default());
        let cache = AuthQueryCache::new(
            "test_pool".to_string(),
            fetcher.clone(),
            &test_config(),
            Some(stats.clone()),
        );

        // Cache miss → executor_queries + cache_misses
        cache.get_or_fetch("alice").await.unwrap();
        assert_eq!(stats.cache_misses.load(Ordering::Relaxed), 1);
        assert_eq!(stats.executor_queries.load(Ordering::Relaxed), 1);
        assert_eq!(stats.cache_hits.load(Ordering::Relaxed), 0);

        // Cache hit
        cache.get_or_fetch("alice").await.unwrap();
        assert_eq!(stats.cache_hits.load(Ordering::Relaxed), 1);
        assert_eq!(stats.executor_queries.load(Ordering::Relaxed), 1); // no new query

        // Refetch
        cache.refetch_on_failure("alice").await.unwrap();
        assert_eq!(stats.cache_refetches.load(Ordering::Relaxed), 1);
        assert_eq!(stats.executor_queries.load(Ordering::Relaxed), 2);

        // Rate-limited refetch (min_interval = 1s, immediately after first refetch)
        cache.refetch_on_failure("alice").await.unwrap();
        assert_eq!(stats.cache_rate_limited.load(Ordering::Relaxed), 1);
        assert_eq!(stats.executor_queries.load(Ordering::Relaxed), 2); // no new query
    }

    // -- parse_startup_parameters_text: pure-parser unit tests --

    #[test]
    fn parse_startup_parameters_absent_column_returns_empty() {
        let r = AuthQueryExecutor::parse_startup_parameters_text(None, "u", "p");
        assert!(r.is_empty());
    }

    #[test]
    fn parse_startup_parameters_empty_string_returns_empty() {
        let r = AuthQueryExecutor::parse_startup_parameters_text(Some(""), "u", "p");
        assert!(r.is_empty());
    }

    #[test]
    fn parse_startup_parameters_simple_json_object() {
        let r = AuthQueryExecutor::parse_startup_parameters_text(
            Some(r#"{"plan_cache_mode":"force_custom_plan","work_mem":"64MB"}"#),
            "u",
            "p",
        );
        assert_eq!(
            r.get("plan_cache_mode").map(String::as_str),
            Some("force_custom_plan")
        );
        assert_eq!(r.get("work_mem").map(String::as_str), Some("64MB"));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn parse_startup_parameters_reserved_key_dropped() {
        // 'user' is reserved by pg_doorman; the valid sibling key survives.
        let r = AuthQueryExecutor::parse_startup_parameters_text(
            Some(r#"{"user":"x","work_mem":"64MB"}"#),
            "u",
            "p",
        );
        assert!(!r.contains_key("user"));
        assert_eq!(r.get("work_mem").map(String::as_str), Some("64MB"));
    }

    #[test]
    fn parse_startup_parameters_non_string_values_dropped() {
        // number, boolean, null, array, object on the right-hand side are
        // all rejected; only string-valued entries survive.
        let r = AuthQueryExecutor::parse_startup_parameters_text(
            Some(
                r#"{"work_mem":64,"on":true,"off":null,"arr":[1],"obj":{},"plan_cache_mode":"force_custom_plan"}"#,
            ),
            "u",
            "p",
        );
        assert_eq!(r.len(), 1);
        assert_eq!(
            r.get("plan_cache_mode").map(String::as_str),
            Some("force_custom_plan")
        );
    }

    #[test]
    fn parse_startup_parameters_malformed_json_returns_empty() {
        let r = AuthQueryExecutor::parse_startup_parameters_text(Some("not-json"), "u", "p");
        assert!(r.is_empty());
    }

    #[test]
    fn parse_startup_parameters_non_object_returns_empty() {
        let r = AuthQueryExecutor::parse_startup_parameters_text(Some("[1,2,3]"), "u", "p");
        assert!(r.is_empty());
    }

    #[test]
    fn parse_startup_parameters_oversize_text_returns_empty() {
        // HIGH #9 regression guard: pathological auth_query row should not
        // make serde_json walk megabytes of JSON. The raw text cap matches
        // `MAX_OPERATOR_BUDGET`, so anything past that returns empty before
        // we even start parsing. Drop the same value into a giant string
        // so the byte length crosses the cap independently of JSON shape.
        let cap = crate::config::startup_parameters::MAX_OPERATOR_BUDGET;
        let bytes = "a".repeat(cap + 1);
        let r = AuthQueryExecutor::parse_startup_parameters_text(Some(&bytes), "u", "p");
        assert!(
            r.is_empty(),
            "oversize raw column must be rejected before serde_json walks it"
        );
    }

    #[test]
    fn parse_startup_parameters_invalid_guc_name_dropped() {
        // Keys with spaces fail the shared `is_valid_guc_name` check used
        // for operator-supplied parameter maps.
        let r = AuthQueryExecutor::parse_startup_parameters_text(
            Some(r#"{"bad name":"x","plan_cache_mode":"force_custom_plan"}"#),
            "u",
            "p",
        );
        assert!(!r.contains_key("bad name"));
        assert!(r.contains_key("plan_cache_mode"));
    }

    #[test]
    fn parse_startup_parameters_null_byte_value_dropped() {
        // A null byte in the value fails the shared validator; the good
        // neighbor still survives.
        let r = AuthQueryExecutor::parse_startup_parameters_text(
            Some("{\"work_mem\":\"64\\u0000MB\",\"plan_cache_mode\":\"force_custom_plan\"}"),
            "u",
            "p",
        );
        assert!(!r.contains_key("work_mem"));
        assert!(r.contains_key("plan_cache_mode"));
    }

    // -- dedicated_mode_filter: drops params + warns once per username --

    #[tokio::test]
    async fn dedicated_mode_filter_drops_params_and_warns_once() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user_with_params("alice", "md5abc123", &[("work_mem", "64MB")]);
        let mut config = test_config();
        // Mark the config as dedicated by providing a server_user.
        config.server_user = Some("doorman_backend".to_string());

        let cache = make_cache(fetcher.clone(), &config);

        // Cache miss path applies the filter: per-user params are dropped
        // because the backend identity is shared in dedicated mode.
        let entry = cache.get_or_fetch("alice").await.unwrap().unwrap();
        assert!(
            entry.startup_parameters.is_empty(),
            "params must be cleared in dedicated mode"
        );

        // The warning fires at most once per username: subsequent calls do
        // not insert into dedicated_warnings again. We assert that the
        // tracker still holds exactly one entry after a second miss-and-fill.
        cache.invalidate("alice");
        let entry = cache.get_or_fetch("alice").await.unwrap().unwrap();
        assert!(entry.startup_parameters.is_empty());
        assert_eq!(cache.dedicated_warnings.len(), 1);

        // clear() resets the warning tracker so a config reload re-arms it.
        cache.clear();
        assert_eq!(cache.dedicated_warnings.len(), 0);
    }

    #[tokio::test]
    async fn non_dedicated_mode_keeps_params() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user_with_params("alice", "md5abc123", &[("work_mem", "64MB")]);
        let config = test_config(); // server_user = None: passthrough mode

        let cache = make_cache(fetcher.clone(), &config);
        let entry = cache.get_or_fetch("alice").await.unwrap().unwrap();
        assert_eq!(
            entry.startup_parameters.get("work_mem").map(String::as_str),
            Some("64MB")
        );
    }

    // ---------------------------------------------------------------------
    // peek_startup_parameters: sync, non-fetching lookup used by backend spawn
    // ---------------------------------------------------------------------

    // Closure-based API tested by snapshotting the borrowed HashMap into
    // an owned one when an existing assertion needs to inspect contents.
    // Generic over the cache's fetcher because the test harness uses a
    // `MockFetcher` rather than the production `AuthQueryExecutor`.
    fn peek_snapshot<F>(
        cache: &AuthQueryCache<F>,
        username: &str,
    ) -> Option<std::collections::HashMap<String, String>>
    where
        F: PasswordFetcher,
    {
        cache.peek_startup_parameters(username, |m| m.clone())
    }

    #[tokio::test]
    async fn peek_startup_parameters_missing_user_returns_none() {
        let fetcher = Arc::new(MockFetcher::new());
        let config = test_config();
        let cache = make_cache(fetcher, &config);
        assert!(peek_snapshot(&cache, "alice").is_none());
    }

    #[tokio::test]
    async fn peek_startup_parameters_negative_entry_returns_none() {
        let fetcher = Arc::new(MockFetcher::new());
        // No user added; first lookup populates a negative cache entry.
        let config = test_config();
        let cache = make_cache(fetcher, &config);
        assert!(cache.get_or_fetch("ghost").await.unwrap().is_none());
        assert!(peek_snapshot(&cache, "ghost").is_none());
    }

    #[tokio::test]
    async fn peek_startup_parameters_returns_none_for_expired_entry() {
        // HIGH #7 regression guard: a positive cache entry that has lived
        // past `cache_ttl` must not pin a stale per-user startup parameter
        // onto a backend the replenishment loop spawns later. Mirrors
        // `test_cache_ttl_expiration` but exercises the peek path the
        // backend-spawn hot path uses.
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user_with_params("alice", "md5abc123", &[("work_mem", "64MB")]);
        let mut config = test_config();
        config.cache_ttl = Duration::from_millis(50);

        let cache = make_cache(fetcher, &config);
        cache.get_or_fetch("alice").await.unwrap().unwrap();
        // Verify that peek sees the entry before it expires.
        assert!(peek_snapshot(&cache, "alice").is_some());

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        assert!(
            peek_snapshot(&cache, "alice").is_none(),
            "peek must return None once cache_ttl has elapsed for the entry"
        );
    }

    #[tokio::test]
    async fn peek_startup_parameters_positive_entry_returns_map() {
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user_with_params(
            "alice",
            "md5abc123",
            &[("work_mem", "64MB"), ("statement_timeout", "10s")],
        );
        let config = test_config();
        let cache = make_cache(fetcher, &config);
        cache.get_or_fetch("alice").await.unwrap().unwrap();

        let params = peek_snapshot(&cache, "alice").unwrap();
        assert_eq!(params.get("work_mem").map(String::as_str), Some("64MB"));
        assert_eq!(
            params.get("statement_timeout").map(String::as_str),
            Some("10s")
        );
    }

    #[tokio::test]
    async fn peek_startup_parameters_dedicated_mode_returns_empty() {
        // Dedicated mode keeps the user cached but removes per-user params.
        let fetcher = Arc::new(MockFetcher::new());
        fetcher.add_user_with_params("alice", "md5abc123", &[("work_mem", "64MB")]);
        let mut config = test_config();
        config.server_user = Some("shared".to_string());
        config.server_password = Some("secret".to_string());

        let cache = make_cache(fetcher, &config);
        cache.get_or_fetch("alice").await.unwrap().unwrap();

        let params = peek_snapshot(&cache, "alice").unwrap();
        assert!(params.is_empty());
    }
}
