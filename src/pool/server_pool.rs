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
use crate::patroni::types::Role;
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
        // `query_wait_timeout` is a soft outer deadline: it bounds how long
        // the client waits, but per-candidate `startup_with_timeout` already
        // guarantees we cannot block on a single hung node. If the outer
        // deadline fires we still return a clean `ConnectError` so the
        // client gets a sanitized FATAL rather than a hang.
        let deadline = self.query_wait_timeout;
        info!(
            "[{}@{}] fallback: local backend unavailable, entering fallback path (deadline={}ms)",
            self.address.username,
            self.address.pool_name,
            deadline.as_millis()
        );
        match tokio::time::timeout(deadline, self.create_fallback_connection_inner()).await {
            Ok(result) => result,
            Err(_) => {
                warn!(
                    "[{}@{}] fallback: outer deadline {}ms exceeded — aborting",
                    self.address.username,
                    self.address.pool_name,
                    deadline.as_millis()
                );
                Err(Error::ConnectError(format!(
                    "fallback total deadline {}ms exceeded",
                    deadline.as_millis()
                )))
            }
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
                info!(
                    "[{}@{}] fallback: whitelist round failed ({err}), retrying with fresh discovery",
                    self.address.username, self.address.pool_name,
                );
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

    /// Run a single fallback round and produce a connection.
    ///
    /// **Two-wave priority race.** Discovery returns every alive member
    /// from `/cluster`; we partition by role and run two waves serially:
    /// 1. **Wave 1 — sync_standby.** Race `Server::startup` against every
    ///    sync_standby in parallel under per-candidate
    ///    `fallback_connect_timeout`. The first Ok wins immediately. The
    ///    user-facing requirement is "sync wins if it's alive at all";
    ///    we do not consider replica/leader while any sync candidate is
    ///    still in-flight, even if a replica would have answered faster.
    /// 2. **Wave 2 — replica + leader.** Only entered if every sync_standby
    ///    failed (or none existed). Race the rest in parallel; first Ok
    ///    wins. Among non-sync candidates we do not preserve replica >
    ///    leader sub-priority — under fallback the system is already in
    ///    a degraded state, fastest live answer is more useful than
    ///    role-based ordering.
    ///
    /// Whitelist-cache hits (`source = WhitelistCache`) skip the wave
    /// machinery and run a single startup against the cached host, since
    /// there's nothing to race against.
    ///
    /// On exhaustion: returns `ConnectError("all fallback candidates
    /// rejected (...)")` with a deterministic per-reason summary. Each
    /// failed candidate is also marked unhealthy (with exponential
    /// backoff) and logged at WARN (rate-limited) or DEBUG.
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

        // Whitelist-cache hit: single target, race-of-one is just a startup.
        if matches!(source, super::fallback::TargetSource::WhitelistCache) {
            let target = match targets.into_iter().next() {
                Some(t) => t,
                None => {
                    return (
                        Err(Error::ConnectError(
                            "whitelist round produced no target".into(),
                        )),
                        source,
                    )
                }
            };
            info!(
                "[{}@{}] fallback: whitelist hit, starting up {}:{} (role={:?})",
                self.address.username,
                self.address.pool_name,
                target.host,
                target.port,
                target.role
            );
            crate::prometheus::FALLBACK_CONNECTIONS_TOTAL
                .with_label_values(&[&self.address.pool_name])
                .inc();
            return match self.try_fallback_target(&target).await {
                Ok(server) => {
                    fallback.set_whitelisted(target.host, target.port, target.role);
                    (Ok(server), source)
                }
                Err(err) => {
                    let reason = super::fallback::FailureReason::from(&err);
                    fallback.mark_unhealthy(&target.host, target.port, reason);
                    (Err(err), source)
                }
            };
        }

        // Discovery: partition candidates into wave 1 (sync_standby) and
        // wave 2 (everything else, in discovery order).
        let (sync_targets, other_targets): (Vec<_>, Vec<_>) = targets
            .into_iter()
            .partition(|t| matches!(t.role, Role::SyncStandby));

        let mut summary = FailureSummary::default();

        // Wave 1.
        if !sync_targets.is_empty() {
            info!(
                "[{}@{}] fallback: wave 1 — racing {} sync_standby candidate(s) ({})",
                self.address.username,
                self.address.pool_name,
                sync_targets.len(),
                format_target_list(&sync_targets),
            );
            if let Some(server) = self
                .race_wave(fallback, &sync_targets, &mut summary, source)
                .await
            {
                return (Ok(server), source);
            }
            info!(
                "[{}@{}] fallback: wave 1 exhausted ({} sync_standby), advancing to wave 2",
                self.address.username,
                self.address.pool_name,
                sync_targets.len(),
            );
        } else {
            info!(
                "[{}@{}] fallback: no sync_standby in cluster, going straight to wave 2",
                self.address.username, self.address.pool_name,
            );
        }

        // Wave 2.
        if !other_targets.is_empty() {
            info!(
                "[{}@{}] fallback: wave 2 — racing {} candidate(s) ({})",
                self.address.username,
                self.address.pool_name,
                other_targets.len(),
                format_target_list(&other_targets),
            );
            if let Some(server) = self
                .race_wave(fallback, &other_targets, &mut summary, source)
                .await
            {
                return (Ok(server), source);
            }
        }

        let summary_str = summary.format();
        warn!(
            "[{}@{}] fallback: all fallback candidates rejected ({summary_str})",
            self.address.username, self.address.pool_name,
        );
        (
            Err(Error::ConnectError(format!(
                "all fallback candidates rejected ({summary_str})"
            ))),
            source,
        )
    }

    /// Race `Server::startup` against `targets` in parallel. On first Ok
    /// return `Some(server)` (winner is whitelisted as a side effect). On
    /// full exhaustion mark every loser unhealthy, record reasons into
    /// `summary`, and return `None` — the caller advances to the next wave
    /// or surfaces the aggregate.
    async fn race_wave(
        &self,
        fallback: &super::fallback::FallbackState,
        targets: &[super::fallback::FallbackTarget],
        summary: &mut FailureSummary,
        source: super::fallback::TargetSource,
    ) -> Option<Server> {
        // We only count "we attempted to use fallback" once per wave, on
        // entry — not per candidate. The metric measures fallback usage
        // pressure, not per-host attempt counts (those live in
        // `_candidate_failures_total`).
        crate::prometheus::FALLBACK_CONNECTIONS_TOTAL
            .with_label_values(&[&self.address.pool_name])
            .inc();
        let _ = source; // reserved for future wave-source-specific logic

        let futures: Vec<futures::future::BoxFuture<'_, Result<Server, Error>>> = targets
            .iter()
            .map(|t| Box::pin(self.try_fallback_target(t)) as _)
            .collect();

        match race_first_success(futures).await {
            Ok((server, idx)) => {
                let winner = &targets[idx];
                info!(
                    "[{}@{}] fallback: winner {}:{} (role={:?}) — startup ok",
                    self.address.username,
                    self.address.pool_name,
                    winner.host,
                    winner.port,
                    winner.role,
                );
                fallback.set_whitelisted(winner.host.clone(), winner.port, winner.role.clone());
                Some(server)
            }
            Err(errors) => {
                for (idx, err) in errors {
                    let target = &targets[idx];
                    let reason = super::fallback::FailureReason::from(&err);
                    fallback.mark_unhealthy(&target.host, target.port, reason);
                    if fallback.should_log_unhealthy(&target.host, target.port) {
                        warn!(
                            "[{}@{}] fallback: {}:{} rejected ({})",
                            self.address.username,
                            self.address.pool_name,
                            target.host,
                            target.port,
                            err
                        );
                    } else {
                        debug!(
                            "[{}@{}] fallback: {}:{} rejected ({}, suppressed)",
                            self.address.username,
                            self.address.pool_name,
                            target.host,
                            target.port,
                            err
                        );
                    }
                    summary.record(err, reason);
                }
                None
            }
        }
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

/// Compact "host:port(role)" list for log lines that summarise a wave's
/// candidate set. Keeps the message readable when the wave has 5+
/// candidates without splitting into multiple lines.
fn format_target_list(targets: &[super::fallback::FallbackTarget]) -> String {
    targets
        .iter()
        .map(|t| format!("{}:{}({:?})", t.host, t.port, t.role))
        .collect::<Vec<_>>()
        .join(", ")
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

/// Race `futures` and return the first `Ok`, together with its index in the
/// input slice. If every future yields `Err`, return all errors with their
/// original indices — the caller decides how to surface them (per-host
/// cooldown, log aggregation). Pending futures are dropped on first
/// success, which cancels the in-flight `Server::startup` for the losing
/// candidates: their TCP sockets go away under us; the kernel finishes the
/// half-open handshake asynchronously. This is intentional — the
/// user-facing requirement is "first successful sync wins", and chasing
/// graceful disconnect on every loser would gate the winner on the slowest
/// loser.
async fn race_first_success<'a, T: 'a, E: 'a>(
    futures: Vec<futures::future::BoxFuture<'a, Result<T, E>>>,
) -> Result<(T, usize), Vec<(usize, E)>> {
    if futures.is_empty() {
        return Err(Vec::new());
    }

    // Bake the original index into each future's output so `select_all`'s
    // own ephemeral index — which renumbers as `rest` shrinks — is not
    // load-bearing.
    let mut indexed: Vec<futures::future::BoxFuture<'a, (usize, Result<T, E>)>> = futures
        .into_iter()
        .enumerate()
        .map(|(i, f)| Box::pin(async move { (i, f.await) }) as _)
        .collect();

    let mut errors: Vec<(usize, E)> = Vec::new();
    while !indexed.is_empty() {
        let ((idx, result), _, rest) = futures::future::select_all(indexed).await;
        match result {
            Ok(value) => return Ok((value, idx)),
            Err(e) => errors.push((idx, e)),
        }
        indexed = rest;
    }

    Err(errors)
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

    // -- race_first_success --------------------------------------------------

    use futures::future::BoxFuture;

    #[tokio::test]
    async fn race_first_success_returns_first_ok_with_index() {
        // The early candidate yields a slow Err, the second yields Ok
        // immediately. Winner index must be 1, value must come from the
        // second future. Pending third future is dropped — required so the
        // test does not stall the runtime for a minute.
        let f0: BoxFuture<'static, Result<&str, &str>> = Box::pin(async {
            tokio::time::sleep(Duration::from_millis(40)).await;
            Err("late err")
        });
        let f1: BoxFuture<'static, Result<&str, &str>> = Box::pin(async { Ok("won") });
        let f2: BoxFuture<'static, Result<&str, &str>> = Box::pin(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok("never reached")
        });

        let (val, idx) = race_first_success(vec![f0, f1, f2])
            .await
            .expect("expected Ok");
        assert_eq!(val, "won");
        assert_eq!(idx, 1);
    }

    #[tokio::test]
    async fn race_first_success_collects_all_errors_when_all_fail() {
        // Every candidate errors. All errors must be collected with their
        // original indices so the caller can mark each candidate unhealthy
        // and aggregate the reasons in the exhaustion log.
        let f0: BoxFuture<'static, Result<&str, &str>> = Box::pin(async { Err("e0") });
        let f1: BoxFuture<'static, Result<&str, &str>> = Box::pin(async { Err("e1") });
        let f2: BoxFuture<'static, Result<&str, &str>> = Box::pin(async { Err("e2") });

        let errs = race_first_success(vec![f0, f1, f2])
            .await
            .expect_err("expected Err");
        assert_eq!(errs.len(), 3);
        let mut indices: Vec<usize> = errs.iter().map(|(i, _)| *i).collect();
        indices.sort();
        assert_eq!(indices, vec![0, 1, 2]);
        // Errors stay attached to their original index, regardless of the
        // order they completed in.
        for (idx, err) in &errs {
            assert_eq!(*err, ["e0", "e1", "e2"][*idx]);
        }
    }

    #[tokio::test]
    async fn race_first_success_empty_input() {
        // Vacuous case: caller must not get a panic on a zero-candidate
        // wave (happens when wave 1 has no sync_standby members).
        let errs: Vec<(usize, &str)> = race_first_success::<&str, &str>(vec![])
            .await
            .expect_err("expected Err");
        assert!(errs.is_empty(), "no candidates → no errors");
    }

    #[tokio::test]
    async fn race_first_success_first_ok_immediately() {
        // First-completed future is the winner even when the second would
        // also have succeeded later.
        let f0: BoxFuture<'static, Result<&str, &str>> = Box::pin(async { Ok("first") });
        let f1: BoxFuture<'static, Result<&str, &str>> = Box::pin(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok("late")
        });

        let (val, idx) = race_first_success(vec![f0, f1]).await.expect("expected Ok");
        assert_eq!(val, "first");
        assert_eq!(idx, 0);
    }
}
