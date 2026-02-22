use dashmap::DashMap;
use log::warn;
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::messages::Parse;
use crate::utils::dashmap::new_dashmap_with_capacity;

/// Global query string interner.
/// This ensures that identical query texts share the same Arc<str> allocation,
/// even when Arc<Parse> is evicted from the pool cache.
/// The interner never evicts entries - they are kept as long as any client holds a reference.
static QUERY_INTERNER: Lazy<DashMap<u64, Arc<str>>> = Lazy::new(|| DashMap::with_capacity(8192));

/// Interns a query string, returning a shared Arc<str>.
/// If the query was already interned, returns the existing Arc<str>.
/// This is used to ensure query texts are shared between all Parse instances.
pub fn intern_query(query: &str, hash: u64) -> Arc<str> {
    // Fast path: check if already interned
    if let Some(existing) = QUERY_INTERNER.get(&hash) {
        return existing.clone();
    }

    // Slow path: intern the query
    let arc_str: Arc<str> = Arc::from(query);
    QUERY_INTERNER.entry(hash).or_insert(arc_str).clone()
}

/// Entry in the prepared statement cache with LRU ordering.
struct CacheEntry {
    parse: Arc<Parse>,
    /// Counter for LRU ordering (higher = more recently used)
    count_used: u64,
}

// TODO: Add stats the this cache
// TODO: Add application name to the cache value to help identify which application is using the cache
// TODO: Create admin command to show which statements are in the cache

/// Concurrent prepared statement cache using DashMap with approximate LRU eviction.
///
/// This implementation provides lock-free reads and fine-grained locking for writes,
/// significantly reducing contention compared to a global Mutex<LruCache>.
pub struct PreparedStatementCache {
    cache: DashMap<u64, CacheEntry>,
    /// Maximum number of entries in the cache
    max_size: usize,
    /// Global counter for LRU ordering
    counter: AtomicU64,
}

impl std::fmt::Debug for PreparedStatementCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedStatementCache")
            .field("size", &self.cache.len())
            .field("max_size", &self.max_size)
            .finish()
    }
}

impl PreparedStatementCache {
    pub fn new(mut size: usize, worker_threads: usize) -> Self {
        // Cannot be zero
        if size == 0 {
            size = 1;
        }

        PreparedStatementCache {
            cache: new_dashmap_with_capacity(size, worker_threads),
            max_size: size,
            counter: AtomicU64::new(0),
        }
    }

    /// Adds the prepared statement to the cache if it doesn't exist with a new name
    /// if it already exists will give you the existing parse
    ///
    /// Pass the hash to this so that we can do the compute before acquiring the lock
    pub fn get_or_insert(&self, parse: &Parse, hash: u64) -> Arc<Parse> {
        let timestamp = self.counter.fetch_add(1, Ordering::Relaxed);

        // Fast path: check if already exists
        if let Some(mut entry) = self.cache.get_mut(&hash) {
            entry.count_used = timestamp;
            return entry.parse.clone();
        }

        // Slow path: insert new entry
        // First intern the query string so it's shared across all clients,
        // then rewrite the statement name
        let new_parse = Arc::new(parse.clone().intern_query(hash).rewrite());

        // Check if we need to evict before inserting
        if self.cache.len() >= self.max_size {
            self.evict_oldest();
        }

        // Insert the new entry
        self.cache.insert(
            hash,
            CacheEntry {
                parse: new_parse.clone(),
                count_used: timestamp,
            },
        );

        new_parse
    }

    /// Returns number of entries in the cache
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Returns true if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Approximate memory usage of the cache in bytes
    pub fn memory_usage(&self) -> usize {
        let mut total = 0;
        for entry in self.cache.iter() {
            total += entry.parse.memory_usage();
            total += std::mem::size_of::<u64>(); // Key
            total += std::mem::size_of::<CacheEntry>();
        }
        total
    }

    /// Returns a list of all entries in the cache
    pub fn get_entries(&self) -> Vec<(u64, Arc<Parse>, u64)> {
        self.cache
            .iter()
            .map(|entry| (*entry.key(), entry.parse.clone(), entry.count_used))
            .collect()
    }

    /// Marks the hash as most recently used if it exists
    pub fn promote(&self, hash: &u64) {
        if let Some(mut entry) = self.cache.get_mut(hash) {
            entry.count_used = self.counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Evict the oldest entry from the cache (approximate LRU).
    fn evict_oldest(&self) {
        // Find the entry with the smallest count_used timestamp
        let mut oldest_key: Option<u64> = None;
        let mut oldest_time = u64::MAX;
        let mut evicted_name: Option<String> = None;

        // Sample entries to find the oldest one
        // We iterate through all entries but this is still efficient because
        // DashMap uses sharding and we only read, not write
        for entry in self.cache.iter() {
            if entry.count_used < oldest_time {
                oldest_time = entry.count_used;
                oldest_key = Some(*entry.key());
            }
        }

        // Remove the oldest entry
        if let Some(key) = oldest_key {
            if let Some((_, entry)) = self.cache.remove(&key) {
                evicted_name = Some(entry.parse.name.clone());
            }
        }

        if let Some(name) = evicted_name {
            warn!("Evicted prepared statement {name} from cache");
        }
    }
}
