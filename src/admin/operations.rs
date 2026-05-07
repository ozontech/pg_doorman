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

/// Outcome of a database-scoped admin action.
///
/// `Applied { affected: 0 }` is legitimate when no `db` filter is given
/// but the pooler holds no pools yet. `NoMatchingDb` is reserved for the
/// case where the caller did pass `db = Some(...)` and no pool matches —
/// transports turn this into a 404 / SQLSTATE 3D000 so the operator gets
/// a clear signal that the typo / stale name took no effect.
#[derive(Debug, PartialEq, Eq)]
pub enum AdminEffect {
    NoMatchingDb { db: String },
    Applied { affected: usize },
}

/// Reload the configuration file. Equivalent to `RELOAD` on the admin
/// protocol; emits the same RELOAD event. Returns `true` when the config
/// actually changed and pools were reconciled, `false` when the file
/// re-parsed identically to the live config (a no-op reload).
pub async fn reload_now() -> Result<bool, Error> {
    let csm = get_client_server_map()
        .ok_or_else(|| Error::SocketError("client_server_map not initialised".into()))?;
    info!("Reloading config (via /api/admin/reload)");
    let changed = reload_config(csm).await?;
    crate::admin::events::push_event("RELOAD", "config reloaded".to_string());
    crate::config::get_config().show();
    Ok(changed)
}

/// Pause every pool whose database segment matches `db`, or every pool
/// when `db` is None.
pub fn pause_now(db: Option<String>) -> AdminEffect {
    apply_per_pool(db, |identifier, pool| {
        pool.database.pause();
        crate::admin::events::push_event("PAUSE", format!("pool {identifier} paused"));
        info!("PAUSE: paused pool {identifier}");
    })
}

/// Resume — mirror of [`pause_now`].
pub fn resume_now(db: Option<String>) -> AdminEffect {
    apply_per_pool(db, |identifier, pool| {
        pool.database.resume();
        crate::admin::events::push_event("RESUME", format!("pool {identifier} resumed"));
        info!("RESUME: resumed pool {identifier}");
    })
}

/// Reconnect — bumps the pool epoch and drains idle connections. Active
/// connections are refused on return.
pub fn reconnect_now(db: Option<String>) -> AdminEffect {
    apply_per_pool(db, |identifier, pool| {
        let new_epoch = pool.database.reconnect();
        crate::admin::events::push_event(
            "RECONNECT",
            format!("pool {identifier} reconnected (epoch={new_epoch})"),
        );
        info!("RECONNECT: reconnected pool {identifier} (new epoch: {new_epoch})");
    })
}

/// Iterate the pool table once: skip pools that do not match the `db`
/// filter, return `NoMatchingDb` if a filter was given and matched
/// nothing, otherwise count how many pools the action ran against.
fn apply_per_pool<F>(db: Option<String>, mut act: F) -> AdminEffect
where
    F: FnMut(&PoolIdentifier, &ConnectionPool),
{
    let pools = get_all_pools();
    if let Some(ref name) = db {
        if !pools.iter().any(|(identifier, _)| identifier.db == *name) {
            return AdminEffect::NoMatchingDb { db: name.clone() };
        }
    }
    let mut affected = 0usize;
    for (identifier, pool) in pools.iter() {
        if let Some(ref name) = db {
            if identifier.db != *name {
                continue;
            }
        }
        act(identifier, pool);
        affected += 1;
    }
    AdminEffect::Applied { affected }
}
