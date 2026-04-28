//! Server connection pool manager.
//!
//! `ServerPool` manages the creation and recycling of individual PostgreSQL
//! server connections. It handles connect timeouts, lifetime checks, alive
//! checks, pause/resume, and reconnect epoch management.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};
use tokio::sync::{Notify, Semaphore};

use crate::config::{Address, User};
use crate::errors::Error;
use crate::server::Server;
use crate::stats::ServerStats;
use crate::utils::format_duration_ms;

use super::errors::{RecycleError, RecycleResult};
use super::types::Metrics;
use super::ClientServerMap;

/// Wrapper for the connection pool.
pub struct ServerPool {
    /// Server address.
    address: Address,

    /// Pool user.
    user: User,

    /// Server database.
    database: String,

    /// Client/server mapping.
    client_server_map: ClientServerMap,

    /// Should we clean up dirty connections before putting them into the pool?
    cleanup_connections: bool,

    application_name: String,

    /// Log client parameter status changes
    log_client_parameter_status_changes: bool,

    /// Prepared statement cache size
    prepared_statement_cache_size: usize,

    /// Semaphore to limit concurrent server connection creation.
    create_semaphore: Arc<Semaphore>,

    /// Counter for total connections created (for logging).
    connection_counter: AtomicU64,

    /// Server lifetime in milliseconds (0 = unlimited).
    lifetime_ms: u64,

    /// Idle timeout in milliseconds (0 = disabled).
    /// Connections idle longer than this are closed by retain.
    idle_timeout_ms: u64,

    /// Time after which idle connections should be checked before reuse (0 = disabled).
    idle_check_timeout_ms: u64,

    /// Connect timeout for alive checks and main-path startup deadline.
    connect_timeout: Duration,

    /// Hard upper bound on how long a single client may wait for a server
    /// connection. Used as the outer deadline around the entire fallback
    /// path: there's no point spending more time than the client itself is
    /// willing to wait. Sourced from `general.query_wait_timeout`.
    query_wait_timeout: Duration,

    /// Session mode flag passed to created Server connections.
    session_mode: bool,

    /// Patroni-assisted fallback state.
    fallback_state: Option<Arc<super::fallback::FallbackState>>,

    /// Combined pool state: bit 32 = paused, bits 0-31 = reconnect epoch (u32).
    pool_state: AtomicU64,

    /// Notify to wake up clients blocked on PAUSE.
    resume_notify: Notify,
}

impl std::fmt::Debug for ServerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerPool")
            .field("address", &self.address)
            .field("user", &self.user)
            .field("database", &self.database)
            .field("cleanup_connections", &self.cleanup_connections)
            .field("application_name", &self.application_name)
            .field(
                "log_client_parameter_status_changes",
                &self.log_client_parameter_status_changes,
            )
            .field(
                "prepared_statement_cache_size",
                &self.prepared_statement_cache_size,
            )
            .field(
                "connection_counter",
                &self.connection_counter.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl ServerPool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        address: Address,
        user: User,
        database: &str,
        client_server_map: ClientServerMap,
        cleanup_connections: bool,
        log_client_parameter_status_changes: bool,
        prepared_statement_cache_size: usize,
        application_name: String,
        max_concurrent_creates: usize,
        lifetime_ms: u64,
        idle_timeout_ms: u64,
        idle_check_timeout_ms: u64,
        connect_timeout: Duration,
        query_wait_timeout: Duration,
        session_mode: bool,
        fallback_state: Option<Arc<super::fallback::FallbackState>>,
    ) -> ServerPool {
        ServerPool {
            address,
            user: user.clone(),
            database: database.to_string(),
            client_server_map,
            cleanup_connections,
            log_client_parameter_status_changes,
            prepared_statement_cache_size,
            create_semaphore: Arc::new(Semaphore::new(max_concurrent_creates)),
            connection_counter: AtomicU64::new(0),
            application_name,
            lifetime_ms,
            idle_timeout_ms,
            idle_check_timeout_ms,
            connect_timeout,
            query_wait_timeout,
            pool_state: AtomicU64::new(0),
            resume_notify: Notify::new(),
            session_mode,
            fallback_state,
        }
    }

    /// Attempts to create a new connection.
    /// Uses a semaphore to limit concurrent connection creation instead of serializing with mutex.
    pub async fn create(&self) -> Result<Server, Error> {
        // Acquire semaphore permit to limit concurrent creates
        let _permit = self
            .create_semaphore
            .acquire()
            .await
            .map_err(|_| Error::ServerStartupReadParameters("Semaphore closed".to_string()))?;

        // Local backend is in cooldown — skip directly to fallback.
        // JustExpired bumps epoch to drain stale fallback connections.
        if let Some(ref fallback) = self.fallback_state {
            use super::fallback::BlacklistCheck;
            match fallback.check_blacklist() {
                BlacklistCheck::Active => {
                    if fallback.should_log_blacklist() {
                        info!(
                            "[{}@{}] fallback: local backend in cooldown, routing to fallback",
                            self.address.username, self.address.pool_name,
                        );
                    } else {
                        debug!(
                            "[{}@{}] fallback: local backend in cooldown, routing to fallback",
                            self.address.username, self.address.pool_name,
                        );
                    }
                    match self.create_fallback_connection().await {
                        Ok(conn) => return Ok(conn),
                        Err(err) => {
                            warn!(
                                "[{}@{}] fallback: connection failed during cooldown: {err}",
                                self.address.username, self.address.pool_name,
                            );
                            // Fall through to try the local backend anyway
                        }
                    }
                }
                BlacklistCheck::JustExpired => {
                    info!(
                        "[{}@{}] fallback: cooldown expired, resuming local backend",
                        self.address.username, self.address.pool_name,
                    );
                    self.bump_epoch();
                }
                BlacklistCheck::NotBlacklisted => {}
            }
        }

        let conn_num = self.connection_counter.fetch_add(1, Ordering::Relaxed) + 1;
        info!(
            "[{}@{}] new server connection #{} to {}:{}",
            self.address.username,
            self.address.pool_name,
            conn_num,
            self.address.host,
            self.address.port,
        );
        let stats = Arc::new(ServerStats::new(
            self.address.clone(),
            crate::utils::clock::now(),
        ));

        stats.register(stats.clone());

        let result = startup_with_timeout(
            self.connect_timeout,
            &self.address.host,
            self.address.port,
            Server::startup(
                &self.address,
                &self.user,
                &self.database,
                self.client_server_map.clone(),
                stats.clone(),
                self.cleanup_connections,
                self.log_client_parameter_status_changes,
                self.prepared_statement_cache_size,
                self.application_name.clone(),
                self.session_mode,
            ),
        )
        .await;

        // libpq sslmode=allow: PostgreSQL has no protocol-level "TLS required"
        // signal — pg_hba rejects plain connections via FATAL 28000 only after
        // StartupMessage. The socket is dead after FATAL, so retry needs a fresh
        // TCP connection. We retry on any startup failure (matching libpq), but
        // skip retry on transport-level errors (ConnectError, ServerUnavailableError)
        // since TLS cannot help when the server was never reached.
        //
        // Reference: PostgreSQL docs, "SSL Support" → sslmode parameter.
        let should_tls_retry = match &result {
            Err(err) if self.address.server_tls.mode.retries_with_tls() => !matches!(
                err,
                Error::ConnectError(_) | Error::ServerUnavailableError(_, _)
            ),
            _ => false,
        };
        let (result, active_stats) = if should_tls_retry {
            info!(
                "plain connection rejected, retrying with tls, user={} pool={} host={} port={} server_tls_mode=allow",
                self.address.username, self.address.pool_name,
                self.address.host, self.address.port,
            );
            // Disconnect the plain-attempt stats before registering the TLS-retry stats.
            // Without this, both entries would remain in SERVER_STATS: the plain one
            // as a ghost if the retry succeeds, or the retry one leaking if it fails.
            stats.disconnect();
            let mut retry_address = self.address.clone();
            retry_address.server_tls = std::sync::Arc::new(crate::config::tls::ServerTlsConfig {
                mode: crate::config::tls::ServerTlsMode::Require,
                connector: self.address.server_tls.connector.clone(),
                cert_hash: self.address.server_tls.cert_hash,
            });
            let retry_stats = Arc::new(ServerStats::new(
                self.address.clone(),
                crate::utils::clock::now(),
            ));
            retry_stats.register(retry_stats.clone());
            let retry_result = startup_with_timeout(
                self.connect_timeout,
                &retry_address.host,
                retry_address.port,
                Server::startup(
                    &retry_address,
                    &self.user,
                    &self.database,
                    self.client_server_map.clone(),
                    retry_stats.clone(),
                    self.cleanup_connections,
                    self.log_client_parameter_status_changes,
                    self.prepared_statement_cache_size,
                    self.application_name.clone(),
                    self.session_mode,
                ),
            )
            .await;
            (retry_result, retry_stats)
        } else {
            (result, stats)
        };

        match result {
            Ok(conn) => {
                // Permit is released automatically when _permit goes out of scope
                conn.stats.idle(0);
                Ok(conn)
            }
            Err(err) => {
                active_stats.disconnect();
                // Local backend unreachable + Patroni-assisted fallback configured: route via fallback.
                if is_backend_unreachable(&err) {
                    if let Some(ref fallback) = self.fallback_state {
                        fallback.blacklist();
                        crate::prometheus::FALLBACK_ACTIVE
                            .with_label_values(&[&self.address.pool_name])
                            .set(1.0);
                        info!(
                            "[{}@{}] fallback: routing through fallback (original error: {err})",
                            self.address.username, self.address.pool_name,
                        );
                        return self.create_fallback_connection().await;
                    }
                }
                // Brief backoff on error to avoid hammering a failing server
                tokio::time::sleep(Duration::from_millis(10)).await;
                Err(err)
            }
        }
    }

    /// Returns the address of this pool.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Establish a fallback connection by iterating through Patroni-discovered
    /// candidates. Per-candidate failures (auth error, "database is starting up",
    /// startup timeout, etc.) mark the candidate unhealthy and proceed to the
    /// next one. Hard-bounded by `query_wait_timeout`: there is no point
    /// spending more time here than the client itself is willing to wait.
    async fn create_fallback_connection(&self) -> Result<Server, Error> {
        let deadline = self.query_wait_timeout;
        match tokio::time::timeout(deadline, self.create_fallback_connection_inner()).await {
            Ok(result) => result,
            Err(_) => Err(Error::ConnectError(format!(
                "fallback total deadline {}ms exceeded",
                deadline.as_millis()
            ))),
        }
    }

    /// Inner body so the outer wrapper can apply the `query_wait_timeout`
    /// guard. Holds the retry-after-stale-whitelist policy.
    async fn create_fallback_connection_inner(&self) -> Result<Server, Error> {
        let fallback = match self.fallback_state.as_ref() {
            Some(fb) => fb,
            None => {
                return Err(Error::ConnectError(
                    "fallback path entered without configured fallback_state".to_string(),
                ));
            }
        };

        let (result, source) = self.run_fallback_round(fallback).await;
        match (result, source) {
            (Ok(conn), _) => Ok(conn),
            (Err(err), super::fallback::TargetSource::WhitelistCache) => {
                // Cached host was stale; wipe it and try with full discovery
                // exactly once more. Bounded retry — discovery round failure
                // surfaces directly without a third try.
                fallback.clear_whitelist();
                let (retry_result, _) = self.run_fallback_round(fallback).await;
                retry_result.map_err(|e2| {
                    Error::ConnectError(format!(
                        "fallback exhausted (whitelist round: {err}; discovery round: {e2})"
                    ))
                })
            }
            (Err(err), super::fallback::TargetSource::Discovery) => Err(err),
        }
    }

    /// Run a single fallback round (whitelist hit or discovery) and try every
    /// alive candidate in priority order. Returns the first successful
    /// connection; on exhaustion returns a `ConnectError` summarising the
    /// failure mix by reason ("3 startup_error, 1 timeout"). Returned
    /// `TargetSource` lets the caller decide whether to retry once more.
    async fn run_fallback_round(
        &self,
        fallback: &super::fallback::FallbackState,
    ) -> (Result<Server, Error>, super::fallback::TargetSource) {
        let (targets, source) = match fallback.get_fallback_targets().await {
            Ok(pair) => pair,
            Err(e) => {
                crate::prometheus::PATRONI_API_ERRORS_TOTAL
                    .with_label_values(&[&self.address.pool_name])
                    .inc();
                warn!(
                    "[{}@{}] fallback: discovery failed: {e}",
                    self.address.username, self.address.pool_name,
                );
                return (
                    Err(Error::ConnectError(format!(
                        "fallback discovery failed: {e}"
                    ))),
                    // Discovery itself failed: no source to speak of, but the
                    // caller treats Discovery as "no retry" — which is what
                    // we want here, the next attempt will be a fresh client
                    // request, not an automatic retry.
                    super::fallback::TargetSource::Discovery,
                );
            }
        };

        let mut summary = FailureSummary::default();
        for target in &targets {
            info!(
                "[{}@{}] fallback: connecting to {}:{} (role: {:?})",
                self.address.username,
                self.address.pool_name,
                target.host,
                target.port,
                target.role,
            );
            crate::prometheus::FALLBACK_CONNECTIONS_TOTAL
                .with_label_values(&[&self.address.pool_name])
                .inc();
            match self.try_fallback_target(target).await {
                Ok(conn) => {
                    fallback.set_whitelisted(target.host.clone(), target.port, target.role.clone());
                    return (Ok(conn), source);
                }
                Err(err) => {
                    let reason = super::fallback::FailureReason::from(&err);
                    fallback.mark_unhealthy(&target.host, target.port, reason);
                    if fallback.should_log_unhealthy(&target.host, target.port) {
                        warn!(
                            "[{}@{}] fallback: {}:{} rejected ({}), trying next",
                            self.address.username,
                            self.address.pool_name,
                            target.host,
                            target.port,
                            err
                        );
                    } else {
                        debug!(
                            "[{}@{}] fallback: {}:{} rejected ({}), trying next",
                            self.address.username,
                            self.address.pool_name,
                            target.host,
                            target.port,
                            err
                        );
                    }
                    summary.record(err, reason);
                }
            }
        }

        let summary_str = summary.format();
        (
            Err(Error::ConnectError(format!(
                "all fallback candidates rejected ({summary_str})"
            ))),
            source,
        )
    }

    /// Attempt a single fallback target with optional sslmode=allow TLS retry.
    /// Returns Ok with a ready Server, or Err mapped from `Server::startup`
    /// (including `ConnectError` on `startup_with_timeout` deadline).
    async fn try_fallback_target(
        &self,
        target: &super::fallback::FallbackTarget,
    ) -> Result<Server, Error> {
        // Use the fallback_connect_timeout for fallback startup deadlines —
        // the same scale as the TCP-probe and per-candidate cooldown window.
        let fallback_timeout = self
            .fallback_state
            .as_ref()
            .map(|fb| fb.connect_timeout())
            .unwrap_or(self.connect_timeout);

        let mut fallback_address = self.address.clone();
        fallback_address.host = target.host.clone();
        fallback_address.port = target.port;

        let stats = Arc::new(ServerStats::new(
            fallback_address.clone(),
            crate::utils::clock::now(),
        ));
        stats.register(stats.clone());

        let result = startup_with_timeout(
            fallback_timeout,
            &fallback_address.host,
            fallback_address.port,
            Server::startup(
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
            ),
        )
        .await;

        // Same sslmode=allow retry as the local-backend path: TLS only when the
        // server rejected us at the protocol level, never on transport failures.
        let should_tls_retry = match &result {
            Err(err) if fallback_address.server_tls.mode.retries_with_tls() => !matches!(
                err,
                Error::ConnectError(_) | Error::ServerUnavailableError(_, _)
            ),
            _ => false,
        };
        let (result, active_stats) = if should_tls_retry {
            info!(
                "[{}@{}] fallback: plain connection to {}:{} rejected, retrying with tls",
                self.address.username,
                self.address.pool_name,
                fallback_address.host,
                fallback_address.port,
            );
            stats.disconnect();
            let mut retry_address = fallback_address.clone();
            retry_address.server_tls = std::sync::Arc::new(crate::config::tls::ServerTlsConfig {
                mode: crate::config::tls::ServerTlsMode::Require,
                connector: fallback_address.server_tls.connector.clone(),
                cert_hash: fallback_address.server_tls.cert_hash,
            });
            let retry_stats = Arc::new(ServerStats::new(
                fallback_address.clone(),
                crate::utils::clock::now(),
            ));
            retry_stats.register(retry_stats.clone());
            let retry_result = startup_with_timeout(
                fallback_timeout,
                &retry_address.host,
                retry_address.port,
                Server::startup(
                    &retry_address,
                    &self.user,
                    &self.database,
                    self.client_server_map.clone(),
                    retry_stats.clone(),
                    self.cleanup_connections,
                    self.log_client_parameter_status_changes,
                    self.prepared_statement_cache_size,
                    self.application_name.clone(),
                    self.session_mode,
                ),
            )
            .await;
            (retry_result, retry_stats)
        } else {
            (result, stats)
        };

        match result {
            Ok(mut conn) => {
                conn.stats.idle(0);
                conn.override_lifetime_ms = Some(target.lifetime_ms);
                Ok(conn)
            }
            Err(err) => {
                active_stats.disconnect();
                Err(err)
            }
        }
    }

    /// Returns the base lifetime in milliseconds for connections in this pool.
    pub fn lifetime_ms(&self) -> u64 {
        self.lifetime_ms
    }

    /// Returns the base idle timeout in milliseconds for connections in this pool.
    pub fn idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms
    }

    /// Bit flag for the paused state within `pool_state`.
    const PAUSED_BIT: u64 = 1 << 32;
    /// Mask for the reconnect epoch (lower 32 bits) within `pool_state`.
    const EPOCH_MASK: u64 = 0xFFFF_FFFF;

    /// Returns whether the pool is paused.
    pub fn is_paused(&self) -> bool {
        self.pool_state.load(Ordering::Acquire) & Self::PAUSED_BIT != 0
    }

    /// Sets the pool as paused.
    pub fn pause(&self) {
        self.pool_state
            .fetch_or(Self::PAUSED_BIT, Ordering::Release);
    }

    /// Resumes the pool and wakes all waiting clients.
    pub fn resume(&self) {
        self.pool_state
            .fetch_and(!Self::PAUSED_BIT, Ordering::Release);
        self.resume_notify.notify_waiters();
    }

    /// Returns a future that completes when the pool is resumed.
    pub fn resume_notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.resume_notify.notified()
    }

    /// Returns the current reconnect epoch.
    pub fn current_epoch(&self) -> u32 {
        (self.pool_state.load(Ordering::Acquire) & Self::EPOCH_MASK) as u32
    }

    /// Increments the reconnect epoch and returns the new value.
    /// Uses CAS loop to modify only the lower 32 bits, preventing
    /// epoch overflow from corrupting PAUSED_BIT at bit 32.
    pub fn bump_epoch(&self) -> u32 {
        loop {
            let old = self.pool_state.load(Ordering::Acquire);
            let old_epoch = (old & Self::EPOCH_MASK) as u32;
            let new_epoch = old_epoch.wrapping_add(1);
            let new = (old & !Self::EPOCH_MASK) | (new_epoch as u64);
            if self
                .pool_state
                .compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return new_epoch;
            }
        }
    }

    /// Checks if the connection can be recycled.
    /// Performs lifetime check and alive check for idle connections.
    ///
    /// `skip_lifetime` lets the caller suppress the `server_lifetime`
    /// expiration check when the pool is under client pressure (no spare
    /// permits, queue forming). Lifetime is housekeeping, not safety: closing
    /// an aged-but-working connection mid-storm forces a `connect()` round-trip
    /// onto the wait path of every queued client. Bad-connection, RECONNECT
    /// epoch and idle-alive checks always run regardless of the flag — those
    /// are correctness, not housekeeping.
    pub async fn recycle(
        &self,
        conn: &mut Server,
        metrics: &Metrics,
        skip_lifetime: bool,
    ) -> RecycleResult {
        if conn.is_bad() {
            conn.close_reason = Some("bad connection".to_string());
            return Err(RecycleError::StaticMessage("Bad connection"));
        }

        // RECONNECT epoch check: reject connections created before current epoch
        if metrics.epoch < self.current_epoch() {
            conn.close_reason = Some("reconnect epoch outdated".to_string());
            return Err(RecycleError::StaticMessage(
                "Connection outdated (RECONNECT)",
            ));
        }

        // Check server_lifetime - applies to all connections, not just idle
        // Uses per-connection lifetime with jitter to prevent mass closures.
        // Skipped when the pool is under pressure: see doc-comment above.
        if let Some(age_ms) = lifetime_exceeded(metrics, skip_lifetime) {
            conn.close_reason = Some(format!(
                "lifetime exceeded (age={}, limit={})",
                format_duration_ms(age_ms),
                format_duration_ms(metrics.lifetime_ms),
            ));
            return Err(RecycleError::StaticMessage("Connection exceeded lifetime"));
        }

        // Check if connection was idle too long and needs alive check
        if self.idle_check_timeout_ms > 0 {
            if let Some(recycled) = metrics.recycled {
                let idle_time_ms = recycled.elapsed().as_millis() as u64;
                if idle_time_ms > self.idle_check_timeout_ms {
                    debug!(
                        "Connection {} idle for {}ms, checking alive...",
                        conn, idle_time_ms
                    );
                    if conn.check_alive(self.connect_timeout).await.is_err() {
                        conn.close_reason = Some(format!(
                            "failed alive check after {} idle",
                            format_duration_ms(idle_time_ms),
                        ));
                        return Err(RecycleError::StaticMessage("Connection failed alive check"));
                    }
                    debug!("Connection {} passed alive check", conn);
                }
            }
        }

        Ok(())
    }
}

fn is_backend_unreachable(err: &Error) -> bool {
    matches!(
        err,
        Error::ConnectError(_) | Error::ServerUnavailableError(_, _)
    )
}

/// Aggregator for per-candidate failure reasons inside one fallback round.
/// Lets `run_fallback_round` build a categorical summary like "3
/// startup_error, 1 timeout" instead of leaking only the last error to the
/// client — operators can tell apart "kernel-level connectivity broken" from
/// "everyone refused on auth" at a glance.
#[derive(Default)]
struct FailureSummary {
    last_err: Option<Error>,
    counts: std::collections::HashMap<super::fallback::FailureReason, u32>,
}

impl FailureSummary {
    fn record(&mut self, err: Error, reason: super::fallback::FailureReason) {
        *self.counts.entry(reason).or_insert(0) += 1;
        self.last_err = Some(err);
    }

    fn format(&self) -> String {
        if self.counts.is_empty() {
            return "no candidates".to_string();
        }
        // Stable ordering so the message is deterministic in logs and tests.
        let mut parts: Vec<(super::fallback::FailureReason, u32)> =
            self.counts.iter().map(|(r, c)| (*r, *c)).collect();
        parts.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        parts
            .into_iter()
            .map(|(r, c)| format!("{c} {}", r.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Wraps a `Server::startup` call in a hard deadline. On timeout returns
/// `Error::ConnectError`, which is treated as transport-level failure: on the
/// main path it triggers fallback, on the fallback path it lets the caller
/// mark the candidate unhealthy and try the next one. Without this, a postgres
/// that opened a TCP socket but never replies to StartupMessage would keep
/// pg_doorman blocked on `read_u8` forever.
async fn startup_with_timeout<F>(
    timeout_duration: Duration,
    host: &str,
    port: u16,
    fut: F,
) -> Result<Server, Error>
where
    F: std::future::Future<Output = Result<Server, Error>>,
{
    match tokio::time::timeout(timeout_duration, fut).await {
        Ok(result) => result,
        Err(_) => Err(Error::ConnectError(format!(
            "server startup timed out to {}:{} after {}ms",
            host,
            port,
            timeout_duration.as_millis()
        ))),
    }
}

/// Returns `Some(age_ms)` when a connection should be closed because it
/// crossed its per-connection `lifetime_ms` budget. Returns `None` when the
/// caller asked to skip the check (`skip_lifetime`), the lifetime budget is
/// disabled (`lifetime_ms == 0`), or the connection is still within budget.
///
/// Pulled out of `recycle()` so the lifetime decision can be exercised
/// without constructing a real `Server` connection in tests.
fn lifetime_exceeded(metrics: &Metrics, skip_lifetime: bool) -> Option<u64> {
    if skip_lifetime || metrics.lifetime_ms == 0 {
        return None;
    }
    let age_ms = metrics.age().as_millis() as u64;
    if age_ms > metrics.lifetime_ms {
        Some(age_ms)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn metrics_with_lifetime(lifetime_ms: u64) -> Metrics {
        Metrics::new(lifetime_ms, 0, 0)
    }

    #[test]
    fn lifetime_exceeded_skipped_when_under_pressure() {
        // A connection well past its budget is kept alive when the caller
        // signals pressure. This is the whole point of the new flag: a
        // working connection must not be closed mid-storm.
        let metrics = metrics_with_lifetime(1);
        thread::sleep(Duration::from_millis(5));
        assert!(lifetime_exceeded(&metrics, true).is_none());
    }

    #[test]
    fn lifetime_exceeded_returns_age_when_age_above_limit() {
        let metrics = metrics_with_lifetime(1);
        // 1ms budget with ±20% jitter resolves to 1ms exactly (jitter floor).
        // Sleep well past it, then assert we report the breach.
        thread::sleep(Duration::from_millis(5));
        let age = lifetime_exceeded(&metrics, false).expect("must exceed lifetime");
        assert!(age >= 1, "reported age {} must be > limit 1", age);
    }

    #[test]
    fn lifetime_exceeded_none_when_lifetime_disabled() {
        // lifetime_ms == 0 means "no budget, never expire", and that
        // contract must hold regardless of the skip flag.
        let metrics = metrics_with_lifetime(0);
        thread::sleep(Duration::from_millis(2));
        assert!(lifetime_exceeded(&metrics, false).is_none());
        assert!(lifetime_exceeded(&metrics, true).is_none());
    }

    #[test]
    fn lifetime_exceeded_none_when_age_within_limit() {
        // Generous budget — connection is fresh, no breach reported.
        let metrics = metrics_with_lifetime(60_000);
        assert!(lifetime_exceeded(&metrics, false).is_none());
    }

    #[tokio::test]
    async fn startup_with_timeout_returns_connect_error_on_deadline() {
        // Simulates a server that opened TCP but never replies to
        // StartupMessage: the inner future never resolves. We expect
        // `startup_with_timeout` to surface this as `ConnectError`, which is
        // what callers treat as a transport-level failure (triggers fallback
        // on the main path; marks the candidate unhealthy on the fallback path).
        let pending = std::future::pending::<Result<Server, Error>>();
        let result =
            startup_with_timeout(Duration::from_millis(20), "1.2.3.4", 5432, pending).await;

        match result {
            Err(Error::ConnectError(msg)) => {
                assert!(
                    msg.contains("startup timed out") && msg.contains("1.2.3.4:5432"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected ConnectError, got: {other:?}"),
        }
    }

    #[test]
    fn failure_summary_format_aggregates_by_reason() {
        use super::super::fallback::FailureReason;
        let mut s = FailureSummary::default();
        s.record(
            Error::ServerStartupError(
                "auth fail".into(),
                crate::errors::ServerIdentifier::new("u".into(), "d", "p"),
            ),
            FailureReason::StartupError,
        );
        s.record(
            Error::ServerStartupError(
                "auth fail".into(),
                crate::errors::ServerIdentifier::new("u".into(), "d", "p"),
            ),
            FailureReason::StartupError,
        );
        s.record(
            Error::ConnectError("timed out".into()),
            FailureReason::Timeout,
        );

        let out = s.format();
        // Stable alphabetic order by reason.as_str(): startup_error < timeout.
        assert_eq!(out, "2 startup_error, 1 timeout");
    }

    #[test]
    fn failure_summary_format_no_candidates_when_empty() {
        let s = FailureSummary::default();
        assert_eq!(s.format(), "no candidates");
    }

    #[tokio::test]
    async fn startup_with_timeout_passes_through_when_inner_resolves() {
        // Successful inner future must not be modified by the wrapper.
        let inner = async {
            Err::<Server, _>(Error::ConnectError(
                "deliberate inner error to assert pass-through".into(),
            ))
        };
        let result = startup_with_timeout(Duration::from_secs(1), "1.2.3.4", 5432, inner).await;

        match result {
            Err(Error::ConnectError(msg)) => {
                assert!(msg.contains("pass-through"), "unexpected message: {msg}");
            }
            other => panic!("expected pass-through ConnectError, got: {other:?}"),
        }
    }
}
