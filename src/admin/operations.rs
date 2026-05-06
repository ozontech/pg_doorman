//! Plain-Rust wrappers around the admin protocol commands so transports
//! other than the postgres-protocol admin socket (the Web UI POST surface
//! at /api/admin/*) can drive the same effects without rendering a fake
//! `RowDescription`/`CommandComplete` reply.
//!
//! Each function is a thin extract of the body of the corresponding handler
//! in `crate::admin::commands`. The original handlers continue to exist —
//! they additionally write the postgres-protocol response envelope to the
//! supplied `AsyncWrite`. Both paths converge on the same pool / config
//! mutations and emit the same `events::push_event` rows so the
//! Web UI's events overlay reflects every triggered action regardless of
//! origin.

use log::info;

use crate::config::reload_config;
use crate::errors::Error;
use crate::pool::{get_all_pools, get_client_server_map};

/// Reload the configuration file. Equivalent to `RELOAD` on the admin
/// protocol; emits the same RELOAD event.
pub async fn reload_now() -> Result<(), Error> {
    let csm = get_client_server_map()
        .ok_or_else(|| Error::SocketError("client_server_map not initialised".into()))?;
    info!("Reloading config (via /api/admin/reload)");
    reload_config(csm).await?;
    crate::admin::events::push_event("RELOAD", "config reloaded".to_string());
    crate::config::get_config().show();
    Ok(())
}

/// Pause one or every pool. `db = Some(name)` filters by database segment;
/// `db = None` pauses every pool.
pub fn pause_now(db: Option<String>) -> usize {
    let pools = get_all_pools();
    let mut affected = 0usize;
    for (identifier, pool) in pools.iter() {
        if let Some(ref name) = db {
            if identifier.db != *name {
                continue;
            }
        }
        pool.database.pause();
        crate::admin::events::push_event("PAUSE", format!("pool {identifier} paused"));
        info!("PAUSE: paused pool {} (via /api/admin)", identifier);
        affected += 1;
    }
    affected
}

/// Resume one or every pool. Mirror of `pause_now`.
pub fn resume_now(db: Option<String>) -> usize {
    let pools = get_all_pools();
    let mut affected = 0usize;
    for (identifier, pool) in pools.iter() {
        if let Some(ref name) = db {
            if identifier.db != *name {
                continue;
            }
        }
        pool.database.resume();
        crate::admin::events::push_event("RESUME", format!("pool {identifier} resumed"));
        info!("RESUME: resumed pool {} (via /api/admin)", identifier);
        affected += 1;
    }
    affected
}

/// Reconnect one or every pool — bumps the pool epoch and drains idle
/// connections. Mirror of the protocol-level RECONNECT.
pub fn reconnect_now(db: Option<String>) -> usize {
    let pools = get_all_pools();
    let mut affected = 0usize;
    for (identifier, pool) in pools.iter() {
        if let Some(ref name) = db {
            if identifier.db != *name {
                continue;
            }
        }
        let new_epoch = pool.database.reconnect();
        crate::admin::events::push_event(
            "RECONNECT",
            format!("pool {identifier} reconnected (epoch={new_epoch})"),
        );
        info!(
            "RECONNECT: reconnected pool {} (new epoch: {}) (via /api/admin)",
            identifier, new_epoch
        );
        affected += 1;
    }
    affected
}
