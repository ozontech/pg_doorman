use dashmap::DashMap;
use log::info;
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Has at least one client ever Parse'd this hash with a non-empty name?
    seen_as_named: AtomicBool,
    /// Has at least one client ever Parse'd this hash with an empty name?
    seen_as_anonymous: AtomicBool,
}

/// Classification of how clients have referenced a pool cache entry over
/// its lifetime. Flags only ever flip from false to true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheEntryKind {
    Named,
    Anonymous,
    Mixed,
}

impl CacheEntryKind {
    fn from_flags(named: bool, anonymous: bool) -> Self {
        match (named, anonymous) {
            (true, true) => CacheEntryKind::Mixed,
            (true, false) => CacheEntryKind::Named,
            (false, true) => CacheEntryKind::Anonymous,
            (false, false) => unreachable!("CacheEntry constructed without a kind flag"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CacheEntryKind::Named => "named",
            CacheEntryKind::Anonymous => "anonymous",
            CacheEntryKind::Mixed => "mixed",
        }
    }
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
    /// Pass the hash to this so that we can do the compute before acquiring the lock.
    /// `client_given_name` is the original Parse name from the client; an empty
    /// string indicates an anonymous prepared statement. The corresponding
    /// `seen_as_*` flag on the entry is flipped from false to true on every call.
    pub fn get_or_insert(&self, parse: &Parse, hash: u64, client_given_name: &str) -> Arc<Parse> {
        let timestamp = self.counter.fetch_add(1, Ordering::Relaxed);
        let is_anonymous = client_given_name.is_empty();

        // Fast path: check if already exists
        if let Some(mut entry) = self.cache.get_mut(&hash) {
            entry.count_used = timestamp;
            if is_anonymous {
                entry.seen_as_anonymous.store(true, Ordering::Relaxed);
            } else {
                entry.seen_as_named.store(true, Ordering::Relaxed);
            }
            return entry.parse.clone();
        }

        // Slow path: insert new entry
        // First intern the query string so it's shared across all clients,
        // then rewrite the statement name
        let new_parse = Arc::new(parse.clone().intern_query(hash).rewrite());

        // Insert first, then evict excess. Reversing the order closes
        // the race where N concurrent callers all pass len() >= max_size
        // before any eviction runs, pushing the cache far above the limit.
        self.cache.insert(
            hash,
            CacheEntry {
                parse: new_parse.clone(),
                count_used: timestamp,
                seen_as_named: AtomicBool::new(!is_anonymous),
                seen_as_anonymous: AtomicBool::new(is_anonymous),
            },
        );

        while self.cache.len() > self.max_size {
            self.evict_oldest();
        }

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

    /// Returns a list of all entries in the cache, including the derived
    /// `CacheEntryKind` reflecting whether clients have used this hash via
    /// named statements, anonymous statements, or both.
    pub fn get_entries(&self) -> Vec<(u64, Arc<Parse>, u64, CacheEntryKind)> {
        self.cache
            .iter()
            .map(|entry| {
                let kind = CacheEntryKind::from_flags(
                    entry.seen_as_named.load(Ordering::Relaxed),
                    entry.seen_as_anonymous.load(Ordering::Relaxed),
                );
                (*entry.key(), entry.parse.clone(), entry.count_used, kind)
            })
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
                let query = entry.parse.query().replace(['\n', '\r'], " ");
                let truncated: String = query.chars().take(80).collect();
                let ellipsis = if query.chars().count() > 80 {
                    "..."
                } else {
                    ""
                };
                info!(
                    "Pool cache eviction: hash={:#x}, name={}, query=\"{truncated}{ellipsis}\", size={}/{}",
                    key, entry.parse.name, self.cache.len(), self.max_size,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{BufMut, BytesMut};
    use std::sync::Arc;

    /// Build a minimal Parse message for testing.
    fn make_parse(name: &str, query: &str) -> Parse {
        let mut buf = BytesMut::new();
        buf.put_u8(b'P');
        let name_bytes = name.as_bytes();
        let query_bytes = query.as_bytes();
        // len = 4 (self) + name + null + query + null + 2 (num_params)
        let len = 4 + name_bytes.len() + 1 + query_bytes.len() + 1 + 2;
        buf.put_i32(len as i32);
        buf.put_slice(name_bytes);
        buf.put_u8(0); // null terminator
        buf.put_slice(query_bytes);
        buf.put_u8(0); // null terminator
        buf.put_i16(0); // no params
        Parse::try_from(&buf).unwrap()
    }

    /// Compute hash the same way callers do.
    fn hash_query(query: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        query.hash(&mut h);
        h.finish()
    }

    /// Concurrent inserts may temporarily overshoot max_size by the number
    /// of concurrent inserters, but must not grow without bound.
    #[test]
    fn concurrent_inserts_bounded_overshoot() {
        let max = 50;
        let cache = Arc::new(PreparedStatementCache::new(max, 4));
        let threads = 20;
        let inserts_per_thread = 10; // total 200 unique inserts into cache of 50

        let barrier = Arc::new(std::sync::Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let cache = cache.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for i in 0..inserts_per_thread {
                        let query = format!("SELECT {} FROM t{}", i, t);
                        let hash = hash_query(&query);
                        let parse = make_parse("stmt", &query);
                        cache.get_or_insert(&parse, hash, "stmt");
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let final_size = cache.len();
        // Overshoot is bounded by the number of concurrent threads.
        // Without the fix, this was 160 (3.2x max_size).
        let allowed = max + threads;
        assert!(
            final_size <= allowed,
            "cache size {} exceeded allowed {} (max_size {} + {} threads)",
            final_size,
            allowed,
            max,
            threads,
        );
    }

    #[test]
    fn flags_named_only_on_named_register() {
        let cache = PreparedStatementCache::new(8, 1);
        let parse = make_parse("stmt_one", "SELECT 1");
        cache.get_or_insert(&parse, 1, "stmt_one");
        let entries = cache.get_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].3, CacheEntryKind::Named);
    }

    #[test]
    fn flags_anonymous_only_on_anonymous_register() {
        let cache = PreparedStatementCache::new(8, 1);
        let parse = make_parse("", "SELECT 1");
        cache.get_or_insert(&parse, 1, "");
        let entries = cache.get_entries();
        assert_eq!(entries[0].3, CacheEntryKind::Anonymous);
    }

    #[test]
    fn flags_mixed_when_both_seen() {
        let cache = PreparedStatementCache::new(8, 1);
        let p1 = make_parse("stmt_one", "SELECT 1");
        cache.get_or_insert(&p1, 1, "stmt_one");
        let p2 = make_parse("", "SELECT 1");
        cache.get_or_insert(&p2, 1, "");
        let entries = cache.get_entries();
        assert_eq!(entries[0].3, CacheEntryKind::Mixed);
    }
}
