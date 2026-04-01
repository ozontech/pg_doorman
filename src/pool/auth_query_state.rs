//! Per-pool auth_query state with lazy executor initialization.
//!
//! The executor and cache are lazily initialized via `OnceCell` on the first
//! auth_query authentication attempt. This ensures `from_config()` never fails
//! due to an unreachable auth_query PostgreSQL server — static users continue
//! working. If the first init fails, `OnceCell` retries on the next call.

use std::sync::Arc;

use log::info;

use crate::auth::auth_query::{AuthQueryCache, AuthQueryExecutor};
use crate::config::AuthQueryConfig;
use crate::errors::Error;
use crate::stats::auth_query::AuthQueryStats;

use super::PoolIdentifier;

/// Per-pool auth_query state: cache + config + shared pool identifier.
pub struct AuthQueryState {
    cache_cell: tokio::sync::OnceCell<AuthQueryCache>,
    pub(crate) config: AuthQueryConfig,
    pool_name: String,
    server_host: String,
    server_port: u16,
    /// Pool identifier for the shared server_user pool (None = passthrough mode).
    pub shared_pool_id: Option<PoolIdentifier>,
    /// Per-pool auth_query metrics (shared with cache and admin/prometheus).
    pub stats: Arc<AuthQueryStats>,
}

impl AuthQueryState {
    /// Create a new AuthQueryState.
    pub(crate) fn new(
        config: AuthQueryConfig,
        pool_name: String,
        server_host: String,
        server_port: u16,
        shared_pool_id: Option<PoolIdentifier>,
        stats: Arc<AuthQueryStats>,
    ) -> Self {
        Self {
            cache_cell: tokio::sync::OnceCell::new(),
            config,
            pool_name,
            server_host,
            server_port,
            shared_pool_id,
            stats,
        }
    }

    /// Get the auth_query config for this pool.
    pub fn config(&self) -> &AuthQueryConfig {
        &self.config
    }

    /// Get the cache, lazily initializing the executor + cache on first access.
    ///
    /// If PG is unreachable, returns `Err`; the `OnceCell` does NOT store the
    /// error, so the next call will retry the connection.
    pub async fn cache(&self) -> Result<&AuthQueryCache, Error> {
        self.cache_cell
            .get_or_try_init(|| async {
                info!(
                    "[pool: {}] auth_query: initializing executor (lazy, first request)",
                    self.pool_name
                );
                let executor = AuthQueryExecutor::new(
                    &self.config,
                    &self.pool_name,
                    &self.server_host,
                    self.server_port,
                )
                .await?;
                Ok(AuthQueryCache::new(
                    Arc::new(executor),
                    &self.config,
                    Some(self.stats.clone()),
                ))
            })
            .await
    }

    /// Clear the auth_query cache if it was already initialized.
    /// Called on RELOAD when auth_query config changes. Does NOT trigger
    /// lazy initialization (safe to call even if executor was never created).
    pub fn try_clear_cache(&self) {
        if let Some(cache) = self.cache_cell.get() {
            cache.clear();
            info!(
                "[pool: {}] auth_query cache cleared on RELOAD",
                self.pool_name
            );
        }
    }

    /// Number of cached entries (0 if cache not yet initialized).
    pub fn cache_len(&self) -> usize {
        self.cache_cell.get().map_or(0, |c| c.len())
    }
}
