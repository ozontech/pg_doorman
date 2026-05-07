//! Short-TTL request snapshot cache. Codex Arch P2#6 / Perf P1#3 flagged
//! that adjacent endpoints (`/api/overview`, `/api/pools`, `/api/clients`,
//! `/api/servers`, `/api/apps`, `/api/stats`) each cloned the global
//! `CLIENT_STATS` / `SERVER_STATS` maps under their own read lock — the
//! same data was walked four to five times per UI poll cycle, and the
//! UI tabs disagreed on which "moment" they were reading.
//!
//! This module exposes a single [`snapshot()`] that returns an
//! `Arc<Snapshot>`. Within a 250 ms TTL window every caller reuses the
//! same `Arc`, so a poll burst from the SPA pays for one snapshot and
//! shares the result. Outside the TTL the next caller rebuilds.
//!
//! Concurrency: the cache is an `ArcSwap`; one thread occasionally
//! rebuilds while others read the previous snapshot, never blocking.
//! After the rebuild the swap is atomic. Older readers keep their
//! `Arc` and finish without observing the change.
//!
//! Memory: at peak we hold two snapshots (the swapped-in current one
//! and any in-flight `Arc`s on stack) — same shape as `arc_swap` does
//! everywhere else in this codebase.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use once_cell::sync::Lazy;

use crate::pool::PoolIdentifier;
use crate::stats::pool::PoolStats;
use crate::stats::{get_client_stats, get_server_stats, ClientStats, ServerStats};

/// One coherent view of pg_doorman's runtime state, suitable for
/// building any `/api/*` envelope without going back to globals.
pub struct Snapshot {
    pub client_states: HashMap<u64, Arc<ClientStats>>,
    pub server_states: HashMap<i32, Arc<ServerStats>>,
    pub pool_lookup: HashMap<PoolIdentifier, PoolStats>,
    pub built_at: Instant,
}

/// 250 ms is roughly twice the SPA's fastest poll interval (1.5 s in
/// the operator console). Big enough that one poll cycle reuses the
/// snapshot across endpoints, small enough that the rendered numbers
/// still feel live to a human watching during an incident.
const TTL: Duration = Duration::from_millis(250);

static CACHE: Lazy<ArcSwap<Option<Arc<Snapshot>>>> = Lazy::new(|| ArcSwap::from_pointee(None));

/// Singleflight gate. A `/api/*` poll burst from one SPA tab brings six
/// adjacent endpoints into [`snapshot()`] within a few microseconds. The
/// first one to find an expired cache enters the critical section and
/// rebuilds; the rest queue on this mutex, then re-check the cache and
/// see fresh data without rebuilding. The mutex is held only across the
/// build itself; readers that find a fresh cache on the fast path never
/// touch it.
static REBUILD_LOCK: Mutex<()> = Mutex::new(());

/// Return the current snapshot, rebuilding it if older than [`TTL`].
pub fn snapshot() -> Arc<Snapshot> {
    if let Some(existing) = CACHE.load().as_ref() {
        if existing.built_at.elapsed() < TTL {
            return existing.clone();
        }
    }
    // Slow path: cache is stale. Serialize rebuilds across concurrent
    // callers — a poll burst should pay for one build, not N. After
    // taking the lock, re-check: another caller may have rebuilt while
    // we were waiting.
    let _guard = REBUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = CACHE.load().as_ref() {
        if existing.built_at.elapsed() < TTL {
            return existing.clone();
        }
    }
    let fresh = Arc::new(build());
    CACHE.store(Arc::new(Some(fresh.clone())));
    fresh
}

fn build() -> Snapshot {
    // Snapshot ordering matches `PoolStats::construct_pool_lookup` —
    // POOLS first, then CLIENT_STATS / SERVER_STATS — so the same race
    // closure (a server orphaned by dynamic-pool GC) applies and the
    // existing benign-orphan logging in `update_client_server_states`
    // covers it.
    let client_states = get_client_stats();
    let server_states = get_server_stats();
    let pool_lookup = PoolStats::construct_pool_lookup_from(&client_states, &server_states);
    Snapshot {
        client_states,
        server_states,
        pool_lookup,
        built_at: Instant::now(),
    }
}
