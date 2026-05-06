//! Pure collection functions for the REST API.
//!
//! Each function reads from project-wide global state (POOLS,
//! get_client_stats(), get_server_stats(), connection counters) and assembles
//! a serializable DTO. Locking is limited to brief Mutex acquisitions for
//! fields that lack a lock-free getter (server application_name).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

mod apps;
mod auth_query;
mod clients;
mod config;
mod connections;
mod databases;
mod events;
mod interner;
mod log_level;
mod overview;
mod pool_coordinator;
mod pool_scaling;
mod pools;
mod prepared;
mod servers;
#[cfg(target_os = "linux")]
mod sockets;
mod stats;
mod top;
mod users;
mod version;

pub(crate) use self::apps::collect_apps;
pub(crate) use self::auth_query::collect_auth_query;
pub(crate) use self::clients::collect_clients;
pub(crate) use self::config::collect_config;
pub(crate) use self::connections::collect_connections;
pub(crate) use self::databases::collect_databases;
pub(crate) use self::events::collect_events;
pub(crate) use self::interner::{collect_interner, collect_interner_top};
pub(crate) use self::log_level::collect_log_level;
pub(crate) use self::overview::collect_overview;
pub(crate) use self::pool_coordinator::collect_pool_coordinator;
pub(crate) use self::pool_scaling::collect_pool_scaling;
pub(crate) use self::pools::collect_pools;
pub(crate) use self::prepared::{collect_prepared, collect_prepared_text};
pub(crate) use self::servers::collect_servers;
#[cfg(target_os = "linux")]
pub(crate) use self::sockets::collect_sockets;
pub(crate) use self::stats::collect_stats;
pub(crate) use self::top::{collect_top_clients, collect_top_prepared, collect_top_queries};
pub(crate) use self::users::collect_users;
pub(crate) use self::version::collect_version;

pub(super) fn cnt(counter: &AtomicUsize) -> u64 {
    counter.load(Ordering::Relaxed) as u64
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// MAX_LIMIT capped at 1000 rows because at typical pooler scale (few thousand
// clients) this is enough for first-page UX; increase if operator feedback
// demands it.
pub(super) const MAX_LIMIT: u64 = 1000;

/// Clamps the user-supplied `?n=` parameter to a sensible range.
///
/// `0` and missing → default 20 (matches SHOW INTERNER TOP convention).
/// Values above 200 are capped — the page would be unusable beyond that
/// and a 100k-entry interner shouldn't materialise an unbounded preview list.
pub(crate) fn clamp_top_n(requested: u64) -> u64 {
    const DEFAULT: u64 = 20;
    const MAX: u64 = 200;
    match requested {
        0 => DEFAULT,
        n if n > MAX => MAX,
        n => n,
    }
}

/// Clamps `?n=` for the Top-N client/apps endpoints. Same shape as
/// `clamp_top_n` for interner top, kept as a separate function so changing
/// the interner cap doesn't affect these page-sized lists.
pub(crate) fn clamp_top_clients_n(requested: u64) -> u64 {
    const DEFAULT: u64 = 20;
    const MAX: u64 = 200;
    match requested {
        0 => DEFAULT,
        n if n > MAX => MAX,
        n => n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_top_n_zero_returns_default() {
        assert_eq!(clamp_top_n(0), 20);
    }

    #[test]
    fn clamp_top_n_keeps_in_range() {
        assert_eq!(clamp_top_n(1), 1);
        assert_eq!(clamp_top_n(50), 50);
        assert_eq!(clamp_top_n(200), 200);
    }

    #[test]
    fn clamp_top_n_caps_above_max() {
        assert_eq!(clamp_top_n(201), 200);
        assert_eq!(clamp_top_n(u64::MAX), 200);
    }

    #[test]
    fn clamp_top_clients_n_zero_returns_default() {
        assert_eq!(clamp_top_clients_n(0), 20);
    }

    #[test]
    fn clamp_top_clients_n_keeps_in_range() {
        assert_eq!(clamp_top_clients_n(1), 1);
        assert_eq!(clamp_top_clients_n(50), 50);
        assert_eq!(clamp_top_clients_n(200), 200);
    }

    #[test]
    fn clamp_top_clients_n_caps_above_max() {
        assert_eq!(clamp_top_clients_n(201), 200);
        assert_eq!(clamp_top_clients_n(u64::MAX), 200);
    }
}
