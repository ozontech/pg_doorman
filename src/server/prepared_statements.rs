use std::sync::Arc;

use lru::LruCache;

use crate::stats::ServerStats;

pub(crate) fn add_to_cache(
    prepared_statement_cache: &mut Option<LruCache<String, ()>>,
    stats: &Arc<ServerStats>,
    name: &str,
) -> Option<String> {
    let cache = match prepared_statement_cache {
        Some(cache) => cache,
        None => return None,
    };

    stats.prepared_cache_add();

    // If we evict something, we need to close it on the server
    if let Some((evicted_name, _)) = cache.push(name.to_string(), ()) {
        if evicted_name != name {
            return Some(evicted_name);
        }
    };

    None
}

pub(crate) fn remove_from_cache(
    prepared_statement_cache: &mut Option<LruCache<String, ()>>,
    stats: &Arc<ServerStats>,
    name: &str,
) {
    let cache = match prepared_statement_cache {
        Some(cache) => cache,
        None => return,
    };

    stats.prepared_cache_remove();
    cache.pop(name);
}

pub(crate) fn has(
    prepared_statement_cache: &Option<LruCache<String, ()>>,
    stats: &Arc<ServerStats>,
    name: &str,
) -> bool {
    let cache = match prepared_statement_cache {
        Some(cache) => cache,
        None => return false,
    };
    let exists = cache.contains(name);
    if exists {
        stats.prepared_cache_hit();
    } else {
        stats.prepared_cache_miss();
    }
    exists
}
