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

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    fn make_stats() -> Arc<ServerStats> {
        Arc::new(ServerStats::default())
    }

    /// Demonstrates the core bug: `has()` uses `contains()` which does NOT
    /// promote entries in the LRU, so an actively-checked statement can still
    /// be the eviction victim when a new statement is added.
    ///
    /// Sequence:
    /// 1. Add A, B to cache (size=2)          → LRU order: [A, B]
    /// 2. has(A)  — does NOT promote A         → LRU order: [A, B]  (unchanged!)
    /// 3. add_to_cache(C) — evicts LRU entry   → evicts A!
    ///
    /// If has() promoted A, the order after step 2 would be [B, A],
    /// and step 3 would evict B instead.
    #[test]
    fn test_has_does_not_promote_in_lru() {
        let stats = make_stats();
        let mut cache = Some(LruCache::new(NonZeroUsize::new(2).unwrap()));

        // Fill cache: A then B
        assert!(add_to_cache(&mut cache, &stats, "DOORMAN_1").is_none());
        assert!(add_to_cache(&mut cache, &stats, "DOORMAN_2").is_none());

        // Check A exists — but this does NOT promote A in LRU
        assert!(has(&cache, &stats, "DOORMAN_1"));

        // Add C — should evict A (the LRU entry) because has() didn't promote it
        let evicted = add_to_cache(&mut cache, &stats, "DOORMAN_3");
        assert_eq!(
            evicted,
            Some("DOORMAN_1".to_string()),
            "BUG CONFIRMED: has() does not promote, so the 'recently checked' A is evicted"
        );
    }

    /// In lru 0.16, push() DOES promote existing entries (unlike some older versions).
    /// Re-adding the same key via push() moves it to the MRU position.
    /// This means add_to_cache() promotes, but has() (contains()) does NOT.
    #[test]
    fn test_push_promotes_existing_in_lru_016() {
        let stats = make_stats();
        let mut cache = Some(LruCache::new(NonZeroUsize::new(2).unwrap()));

        // Fill cache: A then B → LRU order: [A, B]
        add_to_cache(&mut cache, &stats, "DOORMAN_1");
        add_to_cache(&mut cache, &stats, "DOORMAN_2");

        // Re-add A via push — in lru 0.16 this DOES promote A
        // LRU order becomes: [B, A]
        add_to_cache(&mut cache, &stats, "DOORMAN_1");

        // Add C — evicts B (now the LRU entry), not A
        let evicted = add_to_cache(&mut cache, &stats, "DOORMAN_3");
        assert_eq!(
            evicted,
            Some("DOORMAN_2".to_string()),
            "push() in lru 0.16 promotes existing entries"
        );
    }

    /// Simulates the exact batch processing bug scenario:
    /// Server LRU has [A, B]. Client batch: Parse(A), Bind(A), Parse(C).
    /// Parse(A) → has(A)=true → skip. Bind(A) → has(A)=true → skip.
    /// Parse(C) → add_to_cache(C) → evicts A → Close(A) sent → A deleted.
    /// But Bind(A) is still in client buffer → "prepared statement does not exist".
    #[test]
    fn test_batch_eviction_scenario() {
        let stats = make_stats();
        let mut cache = Some(LruCache::new(NonZeroUsize::new(2).unwrap()));

        // Setup: server has A and B (like after a previous transaction)
        add_to_cache(&mut cache, &stats, "DOORMAN_1"); // statement A
        add_to_cache(&mut cache, &stats, "DOORMAN_2"); // statement B

        // Client sends Parse(A) → has_prepared_statement("DOORMAN_1")
        assert!(has(&cache, &stats, "DOORMAN_1"), "A should exist");

        // Client sends Bind(A) → ensure_on_server → has_prepared_statement("DOORMAN_1")
        assert!(has(&cache, &stats, "DOORMAN_1"), "A should still exist");
        // Bind(A) is now in the client buffer

        // Client sends Parse(C) → not on server → register → add_to_cache
        let evicted = add_to_cache(&mut cache, &stats, "DOORMAN_3");

        // A is evicted — Close(A) would be sent to PostgreSQL
        assert_eq!(evicted, Some("DOORMAN_1".to_string()));

        // After eviction: A no longer exists on the server
        assert!(!has(&cache, &stats, "DOORMAN_1"), "A was evicted");
        assert!(has(&cache, &stats, "DOORMAN_3"), "C should exist");

        // But Bind(A) is still in the client buffer!
        // When Sync flushes the buffer, PostgreSQL will reject Bind(DOORMAN_1)
        // with "prepared statement DOORMAN_1 does not exist"
    }
}
