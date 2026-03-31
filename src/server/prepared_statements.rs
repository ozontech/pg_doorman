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
    prepared_statement_cache: &mut Option<LruCache<String, ()>>,
    stats: &Arc<ServerStats>,
    name: &str,
) -> bool {
    let cache = match prepared_statement_cache {
        Some(cache) => cache,
        None => return false,
    };
    // Use get() instead of contains() to promote the entry in the LRU.
    // contains() leaves the entry at its old position, so actively-checked
    // statements could be evicted by a subsequent add_to_cache() call.
    let exists = cache.get(name).is_some();
    if exists {
        stats.prepared_cache_hit();
    } else {
        stats.prepared_cache_miss();
    }
    exists
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    fn make_stats() -> Arc<ServerStats> {
        Arc::new(ServerStats::default())
    }

    /// After fix: has() now uses get() which DOES promote entries.
    /// Checking a statement moves it to MRU, protecting it from eviction.
    #[test]
    fn test_has_promotes_in_lru() {
        let stats = make_stats();
        let mut cache = Some(LruCache::new(NonZeroUsize::new(2).unwrap()));

        // Fill cache: A then B → LRU order: [A, B]
        assert!(add_to_cache(&mut cache, &stats, "DOORMAN_1").is_none());
        assert!(add_to_cache(&mut cache, &stats, "DOORMAN_2").is_none());

        // has(A) now promotes A → LRU order: [B, A]
        assert!(has(&mut cache, &stats, "DOORMAN_1"));

        // Add C → evicts B (LRU), NOT A
        let evicted = add_to_cache(&mut cache, &stats, "DOORMAN_3");
        assert_eq!(
            evicted,
            Some("DOORMAN_2".to_string()),
            "has() promotes A, so B (not A) is evicted"
        );
    }

    /// push() in lru 0.16 also promotes existing entries.
    #[test]
    fn test_push_promotes_existing_in_lru_016() {
        let stats = make_stats();
        let mut cache = Some(LruCache::new(NonZeroUsize::new(2).unwrap()));

        add_to_cache(&mut cache, &stats, "DOORMAN_1");
        add_to_cache(&mut cache, &stats, "DOORMAN_2");

        // Re-add A via push → promotes A → LRU order: [B, A]
        add_to_cache(&mut cache, &stats, "DOORMAN_1");

        let evicted = add_to_cache(&mut cache, &stats, "DOORMAN_3");
        assert_eq!(
            evicted,
            Some("DOORMAN_2".to_string()),
            "push() in lru 0.16 promotes existing entries"
        );
    }

    /// After fix: the batch scenario works because has() promotes A.
    /// Parse(A) → has(A) promotes → Bind(A) → has(A) promotes →
    /// Parse(C) → add_to_cache(C) → evicts B (not A).
    #[test]
    fn test_batch_eviction_scenario_fixed() {
        let stats = make_stats();
        let mut cache = Some(LruCache::new(NonZeroUsize::new(2).unwrap()));

        add_to_cache(&mut cache, &stats, "DOORMAN_1"); // A
        add_to_cache(&mut cache, &stats, "DOORMAN_2"); // B

        // Parse(A) + Bind(A): has() promotes A
        assert!(has(&mut cache, &stats, "DOORMAN_1"));
        assert!(has(&mut cache, &stats, "DOORMAN_1"));

        // Parse(C): evicts B (LRU), not A
        let evicted = add_to_cache(&mut cache, &stats, "DOORMAN_3");
        assert_eq!(evicted, Some("DOORMAN_2".to_string()));

        // A still exists — Bind(A) in buffer will succeed
        assert!(has(&mut cache, &stats, "DOORMAN_1"));
    }
}
