# Step 3: AuthQueryCache

## Goal

Implement the caching layer with per-username locking (double-checked locking
pattern from Problem 2 decision) and TTL-based invalidation.

## Dependencies

- Step 2 (AuthQueryExecutor — needed for cache miss → fetch)

## 3.1 Cache struct

### File: `src/auth/auth_query.rs` (extend existing)

```rust
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex as TokioMutex;

/// Single cache entry for a username's credentials.
#[derive(Clone, Debug)]
pub struct CacheEntry {
    /// Password hash from pg_shadow ("md5..." or "SCRAM-SHA-256$...")
    pub password_hash: String,
    /// When this entry was fetched from PG
    pub fetched_at: Instant,
    /// True if user was NOT found in pg_shadow
    pub is_negative: bool,
    /// When was the last re-fetch attempted for this user (rate limiting)
    pub last_refetch_at: Option<Instant>,
}

impl CacheEntry {
    fn positive(password_hash: String) -> Self {
        Self {
            password_hash,
            fetched_at: Instant::now(),
            is_negative: false,
            last_refetch_at: None,
        }
    }

    fn negative() -> Self {
        Self {
            password_hash: String::new(),
            fetched_at: Instant::now(),
            is_negative: true,
            last_refetch_at: None,
        }
    }

    fn is_expired(&self, ttl_secs: u64, failure_ttl_secs: u64) -> bool {
        let ttl = if self.is_negative { failure_ttl_secs } else { ttl_secs };
        self.fetched_at.elapsed().as_secs() >= ttl
    }
}

/// Per-pool auth query cache.
pub struct AuthQueryCache {
    /// Cached credentials keyed by actual username (Odyssey #541 lesson).
    entries: DashMap<String, CacheEntry>,

    /// Per-username locks for request coalescing (Problem 2 decision).
    /// First request acquires lock + fetches; others wait + get cache hit.
    locks: DashMap<String, Arc<TokioMutex<()>>>,

    /// Reference to executor for cache miss → PG fetch.
    executor: Arc<AuthQueryExecutor>,

    /// Config for TTLs and rate limiting.
    cache_ttl: u64,
    cache_failure_ttl: u64,
    min_interval: u64,
}

impl AuthQueryCache {
    pub fn new(executor: Arc<AuthQueryExecutor>, config: &AuthQueryConfig) -> Self {
        Self {
            entries: DashMap::new(),
            locks: DashMap::new(),
            executor,
            cache_ttl: config.cache_ttl,
            cache_failure_ttl: config.cache_failure_ttl,
            min_interval: config.min_interval,
        }
    }

    /// Get password hash for username. Uses cache with double-checked locking.
    ///
    /// Returns:
    /// - Ok(Some(entry)) — user found (positive cache or fresh fetch)
    /// - Ok(None) — user not found (negative cache or fresh fetch returned 0 rows)
    /// - Err — executor error (PG down, SQL error, etc.)
    pub async fn get_or_fetch(&self, username: &str) -> Result<Option<CacheEntry>, Error> {
        // Fast path: check cache without lock
        if let Some(entry) = self.entries.get(username) {
            if !entry.is_expired(self.cache_ttl, self.cache_failure_ttl) {
                return if entry.is_negative { Ok(None) } else { Ok(Some(entry.clone())) };
            }
        }

        // Slow path: acquire per-username lock
        let lock = self.locks
            .entry(username.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();

        let _guard = lock.lock().await;

        // Double-check after acquiring lock
        if let Some(entry) = self.entries.get(username) {
            if !entry.is_expired(self.cache_ttl, self.cache_failure_ttl) {
                return if entry.is_negative { Ok(None) } else { Ok(Some(entry.clone())) };
            }
        }

        // Cache miss — fetch from PG
        match self.executor.fetch_password(username).await? {
            Some((_user, password_hash)) => {
                let entry = CacheEntry::positive(password_hash);
                self.entries.insert(username.to_string(), entry.clone());
                Ok(Some(entry))
            }
            None => {
                let entry = CacheEntry::negative();
                self.entries.insert(username.to_string(), entry);
                Ok(None)
            }
        }
    }

    /// Invalidate cache entry for a username.
    /// Called on auth failure to trigger re-fetch on next attempt.
    pub fn invalidate(&self, username: &str) {
        self.entries.remove(username);
    }

    /// Attempt re-fetch after auth failure (password may have changed).
    /// Returns Ok(Some(entry)) if re-fetched, Ok(None) if rate-limited or user gone.
    ///
    /// Rate limiting: won't re-fetch if last re-fetch was < min_interval ago.
    pub async fn refetch_on_failure(
        &self,
        username: &str,
    ) -> Result<Option<CacheEntry>, Error> {
        // Check rate limit
        if let Some(entry) = self.entries.get(username) {
            if let Some(last) = entry.last_refetch_at {
                if last.elapsed().as_secs() < self.min_interval {
                    return Ok(None); // Rate limited — reject
                }
            }
        }

        // Fetch fresh from PG
        let result = self.executor.fetch_password(username).await?;

        match result {
            Some((_user, password_hash)) => {
                let mut entry = CacheEntry::positive(password_hash);
                entry.last_refetch_at = Some(Instant::now());
                self.entries.insert(username.to_string(), entry.clone());
                Ok(Some(entry))
            }
            None => {
                let mut entry = CacheEntry::negative();
                entry.last_refetch_at = Some(Instant::now());
                self.entries.insert(username.to_string(), entry);
                Ok(None)
            }
        }
    }

    /// Clear all entries (called on RELOAD when auth_query config changes).
    pub fn clear(&self) {
        self.entries.clear();
        self.locks.clear();  // Safe: no one holds locks during RELOAD
    }

    /// Number of cached entries (for metrics/admin).
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
```

## 3.2 Lock map memory concern

The `locks` DashMap grows with unique usernames. To prevent memory leaks from
very long usernames (security edge case), add a username length check BEFORE
hitting the cache:

```rust
const MAX_USERNAME_LEN: usize = 128; // PG NAMEDATALEN is 64

pub async fn get_or_fetch(&self, username: &str) -> Result<Option<CacheEntry>, Error> {
    if username.len() > MAX_USERNAME_LEN {
        return Ok(None); // Treat as "user not found", don't cache
    }
    // ... existing logic
}
```

Stale lock entries (for users that never reconnect) are cleaned up when `clear()`
is called on RELOAD. For normal operation, the overhead is negligible — each
lock entry is an Arc<Mutex<()>> ≈ 64 bytes.

## 3.3 Unit tests

Tests (can use a mock executor or in-memory fake):

- `test_cache_hit` — insert entry, get_or_fetch returns cached
- `test_cache_miss_fetches` — empty cache → calls executor
- `test_cache_ttl_expiration` — entry older than cache_ttl → re-fetched
- `test_negative_cache` — user not found → cached for cache_failure_ttl
- `test_invalidate` — invalidate() removes entry
- `test_rate_limiting` — refetch_on_failure within min_interval → returns None
- `test_double_checked_locking` — concurrent get_or_fetch for same user → 1 fetch
- `test_long_username_rejected` — >128 chars → None without caching

## Checklist

- [ ] `CacheEntry` struct
- [ ] `AuthQueryCache` struct with DashMap + per-username locks
- [ ] `get_or_fetch()` with double-checked locking
- [ ] `invalidate()` for auth failure
- [ ] `refetch_on_failure()` with rate limiting
- [ ] `clear()` for RELOAD
- [ ] Username length guard
- [ ] Unit tests (8+)
- [ ] `cargo fmt && cargo clippy -- --deny "warnings"`
