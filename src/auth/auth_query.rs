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

/// Trait for fetching password hashes from PostgreSQL.
/// `AuthQueryExecutor` implements this; tests and benchmarks can substitute a mock.
pub trait PasswordFetcher: Send + Sync {
    fn fetch<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = Result<Option<String>, Error>> + Send + 'a;
}

impl PasswordFetcher for AuthQueryExecutor {
    fn fetch<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = Result<Option<String>, Error>> + Send + 'a {
        self.fetch_password(username)
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

        let (tx, rx) = mpsc::channel(config.credential_lookup_pool_size as usize);

        for i in 0..config.credential_lookup_pool_size {
            info!(
                "[pool: {pool_name}] auth_query: opening executor connection {}/{} \
                 to {server_host}:{server_port}/{database} as '{}'",
                i + 1,
                config.credential_lookup_pool_size,
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
             {}@{server_host}:{server_port}/{database} (credential_lookup_pool_size={})",
            config.user, config.credential_lookup_pool_size
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
    /// Returns `Some(password_hash)` or `None` if user not found.
    pub async fn fetch_password(&self, username: &str) -> Result<Option<String>, Error> {
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
            Ok(Some(_)) => {
                debug!(
                    "[pool: {}] auth_query: user '{username}' found ({elapsed:.1?})",
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
    ) -> Result<Option<String>, Error> {
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
            1 => Self::extract_password(&rows[0], username),
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
                warn!("auth_query: user '{username}' has NULL or empty password");
                Ok(None)
            }
        }
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
}

impl CacheEntry {
    fn positive(password_hash: String) -> Self {
        Self {
            password_hash,
            fetched_at: Instant::now(),
            is_negative: false,
            last_refetch_at: None,
            client_key: None,
        }
    }

    fn negative() -> Self {
        Self {
            password_hash: String::new(),
            fetched_at: Instant::now(),
            is_negative: true,
            last_refetch_at: None,
            client_key: None,
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
    /// Cached credentials keyed by username.
    entries: DashMap<String, CacheEntry>,
    /// Per-username locks for request coalescing.
    /// First request acquires lock + fetches; others wait + get cache hit.
    locks: DashMap<String, Arc<TokioMutex<()>>>,
    /// Fetcher for cache miss → PG fetch.
    executor: Arc<F>,
    /// TTL for positive cache entries (user found).
    cache_ttl: Duration,
    /// TTL for negative cache entries (user not found).
    cache_failure_ttl: Duration,
    /// Minimum interval between re-fetches (rate limiting).
    min_interval: Duration,
    /// Optional stats for observability (None in unit tests).
    stats: Option<Arc<AuthQueryStats>>,
}

impl<F: PasswordFetcher> AuthQueryCache<F> {
    pub fn new(
        executor: Arc<F>,
        config: &AuthQueryConfig,
        stats: Option<Arc<AuthQueryStats>>,
    ) -> Self {
        Self {
            entries: DashMap::new(),
            locks: DashMap::new(),
            executor,
            cache_ttl: config.cache_ttl,
            cache_failure_ttl: config.cache_failure_ttl,
            min_interval: config.min_interval,
            stats,
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
                "auth_query cache: rejecting username of length {} (max {MAX_USERNAME_LEN})",
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

        // Cache miss — fetch from PG
        self.inc(|s| &s.executor_queries);
        match self.executor.fetch(username).await {
            Ok(Some(password_hash)) => {
                self.inc(|s| &s.cache_misses);
                let entry = CacheEntry::positive(password_hash);
                self.entries.insert(username.to_string(), entry.clone());
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
            info!("auth_query cache: invalidated entry for '{username}'");
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
                        "auth_query cache: refetch rate-limited for '{username}' ({:.1?} since last)",
                        last.elapsed()
                    );
                    return Ok(None); // Rate limited
                }
            }
        }

        // Fetch fresh from PG
        self.inc(|s| &s.executor_queries);
        self.inc(|s| &s.cache_refetches);
        match self.executor.fetch(username).await {
            Ok(Some(password_hash)) => {
                let mut entry = CacheEntry::positive(password_hash);
                entry.last_refetch_at = Some(Instant::now());
                self.entries.insert(username.to_string(), entry.clone());
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
    pub fn clear(&self) {
        self.entries.clear();
        self.locks.clear();
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
        fetch_count: AtomicUsize,
        /// Optional delay to simulate slow PG queries (for concurrency tests).
        delay: std::time::Duration,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                responses: DashMap::new(),
                fetch_count: AtomicUsize::new(0),
                delay: std::time::Duration::ZERO,
            }
        }

        fn with_delay(delay: std::time::Duration) -> Self {
            Self {
                responses: DashMap::new(),
                fetch_count: AtomicUsize::new(0),
                delay,
            }
        }

        fn add_user(&self, username: &str, password_hash: &str) {
            self.responses
                .insert(username.to_string(), Some(password_hash.to_string()));
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
    }

    fn test_config() -> AuthQueryConfig {
        AuthQueryConfig {
            query: String::new(),
            user: String::new(),
            password: String::new(),
            database: None,
            credential_lookup_pool_size: 1,
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
        AuthQueryCache::new(fetcher, config, None)
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
        let cache = AuthQueryCache::new(fetcher.clone(), &test_config(), Some(stats.clone()));

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
}
