//! Server connection pool manager.
//!
//! `ServerPool` manages the creation and recycling of individual PostgreSQL
//! server connections. It handles connect timeouts, lifetime checks, alive
//! checks, pause/resume, and reconnect epoch management.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info};
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

    /// Connect timeout for alive checks.
    connect_timeout: Duration,

    /// Session mode flag passed to created Server connections.
    session_mode: bool,

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
        session_mode: bool,
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
            pool_state: AtomicU64::new(0),
            resume_notify: Notify::new(),
            session_mode,
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

        // Connect to the PostgreSQL server.
        let result = Server::startup(
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
        )
        .await;

        // libpq sslmode=allow retry: try plain first, retry with TLS on any failure.
        //
        // PostgreSQL has no protocol-level "TLS required" signal. The server
        // rejects non-TLS connections via pg_hba.conf AFTER StartupMessage
        // with FATAL 28000 ("no pg_hba.conf entry ... no encryption").
        // The connection is dead after FATAL, so retry requires a new TCP socket.
        //
        // We retry on ANY startup failure (not just SSL-related) because:
        // 1. libpq does the same: "first try a non-SSL connection; if that fails,
        //    try an SSL connection" — no message parsing.
        // 2. If the real error is unrelated to TLS (wrong password, DB not found),
        //    the TLS retry will fail with the same error, which we then return.
        //
        // Reference: PostgreSQL docs, "SSL Support" → sslmode parameter.
        let result = if result.is_err() && self.address.server_tls.mode.retries_with_tls() {
            info!(
                "plain connection failed, retrying with tls, user={} pool={} host={} port={} server_tls_mode=allow",
                self.address.username, self.address.pool_name,
                self.address.host, self.address.port,
            );
            let mut retry_address = self.address.clone();
            retry_address.server_tls = std::sync::Arc::new(crate::config::tls::ServerTlsConfig {
                mode: crate::config::tls::ServerTlsMode::Require,
                connector: self.address.server_tls.connector.clone(),
            });
            let retry_stats = Arc::new(ServerStats::new(
                self.address.clone(),
                crate::utils::clock::now(),
            ));
            retry_stats.register(retry_stats.clone());
            Server::startup(
                &retry_address,
                &self.user,
                &self.database,
                self.client_server_map.clone(),
                retry_stats,
                self.cleanup_connections,
                self.log_client_parameter_status_changes,
                self.prepared_statement_cache_size,
                self.application_name.clone(),
                self.session_mode,
            )
            .await
        } else {
            result
        };

        match result {
            Ok(conn) => {
                // Permit is released automatically when _permit goes out of scope
                conn.stats.idle(0);
                Ok(conn)
            }
            Err(err) => {
                // Brief backoff on error to avoid hammering a failing server
                tokio::time::sleep(Duration::from_millis(10)).await;
                stats.disconnect();
                Err(err)
            }
        }
    }

    /// Returns the address of this pool.
    pub fn address(&self) -> &Address {
        &self.address
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
}
