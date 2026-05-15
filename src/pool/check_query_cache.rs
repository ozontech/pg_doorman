//! Per-`ConnectionPool` cache for the response to `general.pooler_check_query`.
//!
//! Holds the last observed `(query, response_bytes)` pair. `get(current)`
//! returns the cached response only when the stored query still matches the
//! caller's current value — a RELOAD that changes `pooler_check_query`
//! self-invalidates the cache on the next probe without any explicit hook
//! into the reload code.

use std::sync::Arc;

use arc_swap::ArcSwap;
use bytes::Bytes;

#[derive(Debug)]
pub struct CheckQueryCache {
    inner: ArcSwap<Option<(String, Bytes)>>,
}

impl CheckQueryCache {
    pub fn new() -> Self {
        Self {
            inner: ArcSwap::from_pointee(None),
        }
    }

    /// Returns `Some(bytes)` when the cache holds a response for `current_query`.
    /// Returns `None` when the cache is empty or the stored query no longer matches.
    pub fn get(&self, current_query: &str) -> Option<Bytes> {
        let snapshot = self.inner.load_full();
        match snapshot.as_ref() {
            Some((q, bytes)) if q == current_query => Some(bytes.clone()),
            _ => None,
        }
    }

    /// Stores a response for `query`. Subsequent `get(current_query)` calls
    /// with `current_query == query` will return `Some(bytes)`.
    pub fn set(&self, query: String, bytes: Bytes) {
        self.inner.store(Arc::new(Some((query, bytes))));
    }
}

impl Default for CheckQueryCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cache_returns_none() {
        let cache = CheckQueryCache::new();
        assert!(cache.get(";").is_none());
        assert!(cache.get("select 1").is_none());
    }

    #[test]
    fn get_after_set_matching_query_returns_bytes() {
        let cache = CheckQueryCache::new();
        cache.set("select 1".to_string(), Bytes::from_static(b"response1"));
        assert_eq!(
            cache.get("select 1"),
            Some(Bytes::from_static(b"response1"))
        );
    }

    #[test]
    fn get_with_different_query_returns_none() {
        let cache = CheckQueryCache::new();
        cache.set("select 1".to_string(), Bytes::from_static(b"response1"));
        assert!(cache.get("select 2").is_none());
        assert!(cache.get(";").is_none());
    }

    #[test]
    fn set_overwrites_previous_value() {
        let cache = CheckQueryCache::new();
        cache.set("select 1".to_string(), Bytes::from_static(b"v1"));
        cache.set("select 2".to_string(), Bytes::from_static(b"v2"));
        assert!(cache.get("select 1").is_none());
        assert_eq!(cache.get("select 2"), Some(Bytes::from_static(b"v2")));
    }

    #[test]
    fn empty_string_query_is_treated_like_any_other() {
        let cache = CheckQueryCache::new();
        cache.set("".to_string(), Bytes::from_static(b"empty"));
        assert_eq!(cache.get(""), Some(Bytes::from_static(b"empty")));
        assert!(cache.get("select 1").is_none());
    }
}
