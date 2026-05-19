//! Single source of truth for the database-scoped admin actions. Both the
//! postgres-protocol admin socket (`crate::admin::commands::{pause,resume,
//! reconnect}`) and the REST surface (`POST /api/admin/{pause,resume,
//! reconnect}`) call into the helpers here and translate the typed
//! [`AdminEffect`] into their own response envelopes. That way the two
//! transports cannot diverge: a `db` filter that matches no pool is
//! reported as a `NoMatchingDb` outcome to both, instead of an SQLSTATE
//! error in one path and a silent `affected: 0` in the other.
//!
//! `events::push_event` is emitted from here, so the Web UI's events
//! overlay paints a marker on every successful action regardless of
//! origin.

use log::info;

use crate::config::reload_config;
use crate::errors::Error;
use crate::pool::{get_all_pools, get_client_server_map, ConnectionPool, PoolIdentifier};

/// Scope filter for `pause` / `resume` / `reconnect`. The REST surface
/// accepts both `?db=<name>` (every user@db pool of one database) and
/// `?pool=<user>@<db>` (one specific pool); the admin protocol path
/// historically only takes a database name, so it always passes
/// [`AdminScope::Database`] or [`AdminScope::AllPools`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum AdminScope {
    AllPools,
    Database(String),
    Pool { user: String, db: String },
}

impl AdminScope {
    fn matches(&self, identifier: &PoolIdentifier) -> bool {
        match self {
            AdminScope::AllPools => true,
            AdminScope::Database(name) => identifier.db == *name,
            AdminScope::Pool { user, db } => identifier.user == *user && identifier.db == *db,
        }
    }
}

/// Outcome of an admin action.
///
/// `Applied { affected: [] }` is legitimate when [`AdminScope::AllPools`]
/// is used but the pooler holds no pools yet. `NoMatchingDb` /
/// `NoMatchingPool` are reserved for the case where the caller did pass
/// a scope filter and no pool matches — transports turn these into a
/// 404 / SQLSTATE 3D000 so the operator gets a clear signal that the
/// typo / stale name took no effect.
///
/// `affected` is the list of touched pools, not just a count, so DBAs
/// can see exactly which `user@db` rows the action ran against — useful
/// when the same database has several users.
#[derive(Debug, PartialEq, Eq)]
pub enum AdminEffect {
    NoMatchingDb { db: String },
    NoMatchingPool { user: String, db: String },
    Applied { affected: Vec<PoolIdentifier> },
}

/// Reload the configuration file. Equivalent to `RELOAD` on the admin
/// protocol; emits the same RELOAD event. Returns `true` when the config
/// actually changed and pools were reconciled, `false` when the file
/// re-parsed identically to the live config (a no-op reload).
pub async fn reload_now() -> Result<bool, Error> {
    let csm = get_client_server_map()
        .ok_or_else(|| Error::SocketError("client_server_map not initialised".into()))?;
    info!("Reloading config (via /api/admin/reload)");
    let changed = match reload_config(csm).await {
        Ok(c) => c,
        Err(e) => {
            crate::admin::events::push_event_rate_limited(
                "CONFIG_VALIDATION_ERROR",
                format!("/api/admin/reload rejected: {e}"),
            );
            return Err(e);
        }
    };
    crate::admin::events::push_event("RELOAD", "config reloaded".to_string());
    crate::config::get_config().show();
    Ok(changed)
}

/// Pause every pool the scope selects.
pub fn pause_now(scope: AdminScope) -> AdminEffect {
    apply_per_pool(scope, |identifier, pool| {
        pool.database.pause();
        crate::admin::events::push_event("PAUSE", format!("pool {identifier} paused"));
        info!("PAUSE: paused pool {identifier}");
    })
}

/// Resume — mirror of [`pause_now`].
pub fn resume_now(scope: AdminScope) -> AdminEffect {
    apply_per_pool(scope, |identifier, pool| {
        pool.database.resume();
        crate::admin::events::push_event("RESUME", format!("pool {identifier} resumed"));
        info!("RESUME: resumed pool {identifier}");
    })
}

/// Reconnect — bumps the pool epoch and drains idle connections. Active
/// connections are refused on return.
pub fn reconnect_now(scope: AdminScope) -> AdminEffect {
    apply_per_pool(scope, |identifier, pool| {
        let new_epoch = pool.database.reconnect();
        crate::admin::events::push_event(
            "RECONNECT",
            format!("pool {identifier} reconnected (epoch={new_epoch})"),
        );
        info!("RECONNECT: reconnected pool {identifier} (new epoch: {new_epoch})");
    })
}

/// Iterate the pool table once: skip pools that do not match the scope,
/// return `NoMatchingDb` / `NoMatchingPool` if the scope's filter
/// matched nothing, otherwise return the list of touched pool ids.
fn apply_per_pool<F>(scope: AdminScope, mut act: F) -> AdminEffect
where
    F: FnMut(&PoolIdentifier, &ConnectionPool),
{
    let pools = get_all_pools();
    if !pools
        .iter()
        .any(|(identifier, _)| scope.matches(identifier))
    {
        return match scope {
            AdminScope::AllPools => AdminEffect::Applied {
                affected: Vec::new(),
            },
            AdminScope::Database(db) => AdminEffect::NoMatchingDb { db },
            AdminScope::Pool { user, db } => AdminEffect::NoMatchingPool { user, db },
        };
    }
    let mut affected = Vec::new();
    for (identifier, pool) in pools.iter() {
        if !scope.matches(identifier) {
            continue;
        }
        act(identifier, pool);
        affected.push(identifier.clone());
    }
    AdminEffect::Applied { affected }
}
