use dashmap::DashMap;
use log::{info, log_enabled, trace, Level};
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::messages::Parse;
use crate::utils::dashmap::new_dashmap_with_capacity;

/// Worker-thread hint for the lazy interner DashMaps. `run_server` calls
/// `set_interner_worker_threads(config.general.worker_threads)` before any
/// client can reach `intern_query`, so the first `Lazy::deref` picks the
/// right shard count via `new_dashmap_with_capacity` (same helper the rest
/// of the project uses to dodge the dashmap-default-cgroup mismatch in
/// k8s deployments). Initialised to 0 so `Lazy::new` outside `run_server`
/// (tests, fallback paths) still gets the helper's `max(1)` floor.
static INTERNER_WORKER_THREADS: AtomicUsize = AtomicUsize::new(0);

/// Setter for the interner shard hint; call once at startup, before any
/// client traffic. Subsequent calls are ignored once the lazy maps have
/// initialised.
pub fn set_interner_worker_threads(worker_threads: usize) {
    INTERNER_WORKER_THREADS.store(worker_threads, Ordering::SeqCst);
}

fn interner_worker_threads() -> usize {
    INTERNER_WORKER_THREADS.load(Ordering::SeqCst).max(1)
}

/// GC bookkeeping flag. Two-cycle mark-and-sweep: a candidate entry is
/// flipped to `MARKED` on one sweep and removed on the next sweep that
/// still sees it as a candidate. Any access between the two sweeps
/// switches the state back to `ACTIVE`, so cold-but-still-needed entries
/// survive the cycle.
const GC_STATE_ACTIVE: u8 = 0;
const GC_STATE_MARKED: u8 = 1;

/// Entry in the named interner. Bounded by passive GC over
/// `Arc::strong_count(text)` — kept as long as any pool/client cache
/// holds a strong reference to the underlying text.
pub struct NamedEntry {
    text: Arc<str>,
    gc_state: AtomicU8,
    /// Cumulative count of Bind events that referenced this hash.
    /// Used by `/api/top/queries?by=count`. Approximate: see plan.
    count: AtomicU64,
    /// Cumulative microseconds spent across all Sync's that ended a batch
    /// whose last Bind referenced this hash. Approximate per-batch
    /// attribution — multi-Bind batches give the entire duration to the
    /// last hash. See plan for the trade-off.
    total_duration_us: AtomicU64,
}

impl NamedEntry {
    fn new(text: Arc<str>) -> Self {
        Self {
            text,
            gc_state: AtomicU8::new(GC_STATE_ACTIVE),
            count: AtomicU64::new(0),
            total_duration_us: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        self.gc_state.store(GC_STATE_ACTIVE, Ordering::Relaxed);
    }

    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    /// Approximate count of Bind references. Used by `/api/top/queries`.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Approximate cumulative execution time in microseconds.
    pub fn total_duration_us(&self) -> u64 {
        self.total_duration_us.load(Ordering::Relaxed)
    }
}

/// Entry in the anonymous interner. Bounded by per-entry TTL over
/// `last_used`; same two-cycle grace period as the named side.
pub struct AnonEntry {
    text: Arc<str>,
    last_used: AtomicU64,
    gc_state: AtomicU8,
    count: AtomicU64,
    total_duration_us: AtomicU64,
}

impl AnonEntry {
    fn new(text: Arc<str>, now_ms: u64) -> Self {
        Self {
            text,
            last_used: AtomicU64::new(now_ms),
            gc_state: AtomicU8::new(GC_STATE_ACTIVE),
            count: AtomicU64::new(0),
            total_duration_us: AtomicU64::new(0),
        }
    }

    fn touch(&self, now_ms: u64) {
        self.last_used.store(now_ms, Ordering::Relaxed);
        self.gc_state.store(GC_STATE_ACTIVE, Ordering::Relaxed);
    }

    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    pub fn idle_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_used.load(Ordering::Relaxed))
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn total_duration_us(&self) -> u64 {
        self.total_duration_us.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn last_used_for_test(&self) -> u64 {
        self.last_used.load(Ordering::Relaxed)
    }
}

/// Global query string interners. Split by `is_anonymous` so the two halves
/// can run different eviction policies (passive `strong_count` GC for named,
/// per-entry TTL for anonymous). The same hash interned both as named and
/// anonymous lives in both maps with independent `Arc<str>` allocations —
/// dedup loss in this rare case is accepted.
static NAMED_INTERNER: Lazy<DashMap<u64, Arc<NamedEntry>>> =
    Lazy::new(|| new_dashmap_with_capacity(8192, interner_worker_threads()));
static ANON_INTERNER: Lazy<DashMap<u64, Arc<AnonEntry>>> =
    Lazy::new(|| new_dashmap_with_capacity(8192, interner_worker_threads()));

/// Monotonic millisecond clock anchored at the first call. Used by
/// `AnonEntry::last_used` so wall-clock jumps don't perturb TTL decisions.
pub fn now_monotonic_ms() -> u64 {
    use std::time::Instant;
    static START: Lazy<Instant> = Lazy::new(Instant::now);
    START.elapsed().as_millis() as u64
}

/// Increments the Bind-count atomic on the interner entry that owns `hash`.
/// No-op if the entry has been GC'd or not yet inserted; we accept the
/// resulting count gap to keep the hot path lock-free.
pub fn record_query_count(hash: u64, is_anonymous: bool) {
    if is_anonymous {
        if let Some(entry) = ANON_INTERNER.get(&hash) {
            entry.count.fetch_add(1, Ordering::Relaxed);
        }
    } else if let Some(entry) = NAMED_INTERNER.get(&hash) {
        entry.count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Adds `micros` to the cumulative duration on the interner entry. Same
/// no-op-on-miss policy as `record_query_count`.
pub fn record_query_duration_us(hash: u64, is_anonymous: bool, micros: u64) {
    if is_anonymous {
        if let Some(entry) = ANON_INTERNER.get(&hash) {
            entry.total_duration_us.fetch_add(micros, Ordering::Relaxed);
        }
    } else if let Some(entry) = NAMED_INTERNER.get(&hash) {
        entry.total_duration_us.fetch_add(micros, Ordering::Relaxed);
    }
}

/// Interns the query string into the matching half of the interner.
/// `is_anonymous` reflects how *this* Parse uses the hash — empty Parse
/// name = anonymous.
pub fn intern_query(query: &str, hash: u64, is_anonymous: bool) -> Arc<str> {
    if is_anonymous {
        intern_anon(query, hash)
    } else {
        intern_named(query, hash)
    }
}

fn intern_named(query: &str, hash: u64) -> Arc<str> {
    if let Some(entry) = NAMED_INTERNER.get(&hash) {
        entry.touch();
        return entry.text.clone();
    }
    let arc_str: Arc<str> = Arc::from(query);
    let new_entry = Arc::new(NamedEntry::new(arc_str.clone()));
    NAMED_INTERNER.entry(hash).or_insert(new_entry).text.clone()
}

fn intern_anon(query: &str, hash: u64) -> Arc<str> {
    let now = now_monotonic_ms();
    if let Some(entry) = ANON_INTERNER.get(&hash) {
        entry.touch(now);
        return entry.text.clone();
    }
    let arc_str: Arc<str> = Arc::from(query);
    let new_entry = Arc::new(AnonEntry::new(arc_str.clone(), now));
    ANON_INTERNER.entry(hash).or_insert(new_entry).text.clone()
}

/// Snapshot of the named interner. Cloning `Arc<NamedEntry>` is cheap;
/// the snapshot is point-in-time and sees concurrent inserts only by luck.
pub fn named_snapshot() -> Vec<(u64, Arc<NamedEntry>)> {
    NAMED_INTERNER
        .iter()
        .map(|e| (*e.key(), e.value().clone()))
        .collect()
}

pub fn anon_snapshot() -> Vec<(u64, Arc<AnonEntry>)> {
    ANON_INTERNER
        .iter()
        .map(|e| (*e.key(), e.value().clone()))
        .collect()
}

pub fn named_len() -> usize {
    NAMED_INTERNER.len()
}

pub fn anon_len() -> usize {
    ANON_INTERNER.len()
}

/// Force-clear both interners. Used by the `RESET INTERNER` admin command.
pub fn reset_interners_force() {
    NAMED_INTERNER.clear();
    ANON_INTERNER.clear();
}

#[cfg(test)]
pub fn reset_interners_for_test() {
    reset_interners_force();
}

#[cfg(test)]
pub fn named_entry_for_test(hash: u64) -> Option<Arc<NamedEntry>> {
    NAMED_INTERNER.get(&hash).map(|e| e.value().clone())
}

#[cfg(test)]
pub fn anon_entry_for_test(hash: u64) -> Option<Arc<AnonEntry>> {
    ANON_INTERNER.get(&hash).map(|e| e.value().clone())
}

/// Result of one GC sweep over a single interner. `marked` counts entries
/// flagged as candidates this cycle; `evicted` counts entries removed
/// because they were already flagged in the previous cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcStats {
    pub marked: u64,
    pub evicted: u64,
    /// Total bytes of interned text alive at the end of the sweep — the
    /// gauge value Prometheus needs without taking a second snapshot.
    pub bytes: u64,
}

/// Apply one mark-and-sweep step to a single entry. Returns the byte
/// length the caller should fold into the live-bytes total.
///
/// Race invariant (do not collapse the two-cycle grace into a single
/// cycle): between the candidacy check and the `swap(MARKED)` a
/// concurrent `intern_query` may clone an Arc and call `touch()`,
/// writing ACTIVE. This sweep then overwrites ACTIVE with MARKED. The
/// next sweep observes either the entry is no longer a candidate (touch
/// path holds the Arc / refreshed last_used) and stores ACTIVE, sparing
/// the entry, or the candidacy still holds and eviction is correct.
/// Removing the grace cycle would let this race evict a freshly-touched
/// entry on the very next allocation.
fn sweep_entry<F>(
    is_candidate: bool,
    gc_state: &AtomicU8,
    text_len: u64,
    stats: &mut GcStats,
    remove: F,
) -> bool
where
    F: FnOnce() -> bool,
{
    if !is_candidate {
        gc_state.store(GC_STATE_ACTIVE, Ordering::Relaxed);
        stats.bytes += text_len;
        return false;
    }
    let prev = gc_state.swap(GC_STATE_MARKED, Ordering::Relaxed);
    if prev == GC_STATE_MARKED && remove() {
        stats.evicted += 1;
        true
    } else if prev == GC_STATE_ACTIVE {
        stats.marked += 1;
        stats.bytes += text_len;
        false
    } else {
        // Already MARKED but `remove` lost the race to a concurrent
        // remove. The entry will not appear in the next snapshot.
        stats.bytes += text_len;
        false
    }
}

/// Mark-and-sweep over `NAMED_INTERNER`. A named entry is a candidate
/// when `Arc::strong_count(text) == 1` — only the interner itself holds
/// the `Arc<str>`. The candidate is marked on cycle N; if it's still a
/// candidate on cycle N+1 (no `intern_query` touched it in between),
/// it's removed. The grace cycle prevents thrash on cold-but-still-needed
/// hashes that would otherwise be reallocated on the very next Parse.
pub fn gc_sweep_named() -> GcStats {
    let mut stats = GcStats::default();
    for (hash, entry) in named_snapshot() {
        let text_len = entry.text.len() as u64;
        sweep_entry(
            Arc::strong_count(&entry.text) == 1,
            &entry.gc_state,
            text_len,
            &mut stats,
            || {
                let removed = NAMED_INTERNER.remove(&hash).is_some();
                if removed && log_enabled!(Level::Trace) {
                    trace!(
                        "query_interner evict named: hash={hash:#x}, bytes={text_len}, query=\"{}\"",
                        truncate_query_for_log(&entry.text)
                    );
                }
                removed
            },
        );
    }
    stats
}

/// Mark-and-sweep over `ANON_INTERNER`. A candidate is an entry whose
/// idle time exceeds `anon_idle_ttl_ms`. Two-cycle grace identical to
/// the named sweep — `intern_query` touch resets the mark. Pass
/// `u64::MAX` to disable TTL eviction (used when the operator sets
/// `query_interner_anon_idle_ttl_seconds = 0`).
pub fn gc_sweep_anon(anon_idle_ttl_ms: u64) -> GcStats {
    let now = now_monotonic_ms();
    let mut stats = GcStats::default();
    for (hash, entry) in anon_snapshot() {
        let text_len = entry.text.len() as u64;
        let idle_ms = entry.idle_ms(now);
        sweep_entry(
            idle_ms > anon_idle_ttl_ms,
            &entry.gc_state,
            text_len,
            &mut stats,
            || {
                let removed = ANON_INTERNER.remove(&hash).is_some();
                if removed && log_enabled!(Level::Trace) {
                    trace!(
                        "query_interner evict anon: hash={hash:#x}, bytes={text_len}, idle_ms={idle_ms}, query=\"{}\"",
                        truncate_query_for_log(&entry.text)
                    );
                }
                removed
            },
        );
    }
    stats
}

/// Maximum number of characters (not bytes — query text may contain
/// multi-byte UTF-8) kept when a query is rendered into a log line.
/// Long queries are truncated with an ellipsis so a runaway statement
/// can't blow up a log shipper.
pub(crate) const LOG_QUERY_MAX_CHARS: usize = 80;

/// Maximum characters preserved in API/admin previews of a query
/// (`/api/top/queries`, `/api/interner`, `SHOW QUERIES`). Wider than the
/// log line because the consumer is interactive UI / `psql` output that
/// can wrap rather than a single-line shipper.
pub const PREVIEW_QUERY_MAX_CHARS: usize = 120;

/// First `PREVIEW_QUERY_MAX_CHARS` characters of `query`, verbatim. No
/// ellipsis, no newline collapse — preview surfaces are expected to
/// render the text as-is so operators can read the original statement.
pub fn preview_query(query: &str) -> String {
    query.chars().take(PREVIEW_QUERY_MAX_CHARS).collect()
}

/// Render a query string into a compact form safe for a single log line:
/// newlines collapsed to spaces (one CR/LF = one space) and trimmed to
/// `LOG_QUERY_MAX_CHARS` characters with a trailing "..." when truncated.
/// Always allocates; the `log` crate's macros already short-circuit
/// argument evaluation below the active level, so a bare `trace!(...)`
/// call is enough — explicit `log_enabled!` guards are only useful on
/// hot paths where avoiding the allocation matters.
pub fn truncate_query_for_log(query: &str) -> String {
    let cleaned = query.replace(['\n', '\r'], " ");
    if cleaned.chars().count() <= LOG_QUERY_MAX_CHARS {
        return cleaned;
    }
    let mut out: String = cleaned.chars().take(LOG_QUERY_MAX_CHARS).collect();
    out.push_str("...");
    out
}

/// Bit set when at least one client has Parse'd this hash with a non-empty name.
const FLAG_NAMED: u8 = 0b01;
/// Bit set when at least one client has Parse'd this hash with an empty name.
const FLAG_ANONYMOUS: u8 = 0b10;

/// Entry in the prepared statement cache with LRU ordering.
struct CacheEntry {
    parse: Arc<Parse>,
    /// Counter for LRU ordering (higher = more recently used)
    count_used: u64,
    /// Bitmask of `CacheEntryKind` flags. Bit 0 = seen as named,
    /// bit 1 = seen as anonymous. At least one bit is always set after
    /// construction (`CacheEntry::new`); bits only ever flip from 0 to 1.
    kind_flags: AtomicU8,
    /// Cumulative count of Parse-time has_prepared_statement(server_name) hits
    /// for this hash. Approximate per-pool counter — see plan for the LRU
    /// eviction caveat.
    hit_count: AtomicU64,
    /// Cumulative count of Parse-time has_prepared_statement(server_name)
    /// misses for this hash.
    miss_count: AtomicU64,
}

impl CacheEntry {
    /// Construct an entry with the bitmask reflecting the initial classification.
    /// `initial_kind` must be `Named` or `Anonymous` at the call site of
    /// `get_or_insert`; `Mixed` is supported for completeness.
    fn new(parse: Arc<Parse>, count_used: u64, initial_kind: CacheEntryKind) -> Self {
        let bits = match initial_kind {
            CacheEntryKind::Named => FLAG_NAMED,
            CacheEntryKind::Anonymous => FLAG_ANONYMOUS,
            CacheEntryKind::Mixed => FLAG_NAMED | FLAG_ANONYMOUS,
        };
        Self {
            parse,
            count_used,
            kind_flags: AtomicU8::new(bits),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
        }
    }

    /// Mark this entry as seen via a named statement. Skips the atomic
    /// fetch_or when the bit is already set, avoiding cache-line ping-pong
    /// on hot cache hits.
    fn note_named(&self) {
        if self.kind_flags.load(Ordering::Relaxed) & FLAG_NAMED == 0 {
            self.kind_flags.fetch_or(FLAG_NAMED, Ordering::Relaxed);
        }
    }

    /// Mark this entry as seen via an anonymous statement. Skips the atomic
    /// fetch_or when the bit is already set.
    fn note_anonymous(&self) {
        if self.kind_flags.load(Ordering::Relaxed) & FLAG_ANONYMOUS == 0 {
            self.kind_flags.fetch_or(FLAG_ANONYMOUS, Ordering::Relaxed);
        }
    }

    fn kind(&self) -> CacheEntryKind {
        CacheEntryKind::from_bits(self.kind_flags.load(Ordering::Relaxed))
    }
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
    /// Decode a bitmask back into a `CacheEntryKind`. The 0 pattern is
    /// structurally unreachable because `CacheEntry::new` always writes
    /// at least one bit; we map it to `Mixed` defensively rather than
    /// panicking.
    fn from_bits(bits: u8) -> Self {
        match bits & (FLAG_NAMED | FLAG_ANONYMOUS) {
            FLAG_NAMED => CacheEntryKind::Named,
            FLAG_ANONYMOUS => CacheEntryKind::Anonymous,
            _ => CacheEntryKind::Mixed,
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
    /// Live cumulative byte cost of every entry currently in `cache`. Kept
    /// in sync with insert/evict so `memory_usage()` answers in one atomic
    /// load instead of walking every entry; the walk version was an O(N)
    /// hotspot on every `/api/pools` poll for instances with large
    /// per-pool prepared caches.
    ///
    /// Approximate, not exact: two threads racing the slow path on the
    /// same hash both `fetch_add` while DashMap keeps a single entry,
    /// leaving a phantom-entry overshoot until the slot eventually
    /// evicts. The pre-existing walk was also racy under concurrent
    /// inserts; this counter trades one shape of approximation for one
    /// that is far cheaper to read.
    total_memory_bytes: AtomicU64,
}

/// Per-entry overhead independent of the Parse content (DashMap key + the
/// CacheEntry record itself). Variable part is `parse.memory_usage()`.
const ENTRY_OVERHEAD_BYTES: usize = std::mem::size_of::<u64>() + std::mem::size_of::<CacheEntry>();

/// Byte cost of a single cache entry built around `parse`. Same shape as
/// the original walk in `memory_usage` so the new incremental counter
/// converges to identical totals.
fn entry_bytes(parse: &Parse) -> u64 {
    (parse.memory_usage() + ENTRY_OVERHEAD_BYTES) as u64
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
            total_memory_bytes: AtomicU64::new(0),
        }
    }

    /// Adds the prepared statement to the cache if it doesn't exist with a new name
    /// if it already exists will give you the existing parse
    ///
    /// Pass the hash to this so that we can do the compute before acquiring the lock.
    /// `client_given_name` is the original Parse name from the client. `None`
    /// indicates an anonymous prepared statement (PostgreSQL's empty Parse
    /// name); `Some(name)` carries the client-supplied identifier. The
    /// corresponding bit in the entry's `kind_flags` bitmask is set on every
    /// call (the test-and-set guard skips the atomic write when the bit is
    /// already set).
    pub fn get_or_insert(
        &self,
        parse: &Parse,
        hash: u64,
        client_given_name: Option<&str>,
    ) -> Arc<Parse> {
        let timestamp = self.counter.fetch_add(1, Ordering::Relaxed);
        let is_anonymous = client_given_name.is_none();

        // Fast path: check if already exists
        if let Some(mut entry) = self.cache.get_mut(&hash) {
            entry.count_used = timestamp;
            if is_anonymous {
                entry.note_anonymous();
            } else {
                entry.note_named();
            }
            return entry.parse.clone();
        }

        // Slow path: insert new entry
        // First intern the query string so it's shared across all clients,
        // then rewrite the statement name
        let new_parse = Arc::new(parse.clone().intern_query(hash, is_anonymous).rewrite());

        let initial_kind = if is_anonymous {
            CacheEntryKind::Anonymous
        } else {
            CacheEntryKind::Named
        };

        // Insert first, then evict excess. Reversing the order closes
        // the race where N concurrent callers all pass len() >= max_size
        // before any eviction runs, pushing the cache far above the limit.
        let inserted_bytes = entry_bytes(&new_parse);
        self.cache.insert(
            hash,
            CacheEntry::new(new_parse.clone(), timestamp, initial_kind),
        );
        self.total_memory_bytes
            .fetch_add(inserted_bytes, Ordering::Relaxed);

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

    /// Approximate memory usage of the cache in bytes. Single atomic load
    /// — kept in sync with `get_or_insert` and `evict_oldest` so the
    /// dashboard polling path does not pay an O(N) walk on every snapshot.
    pub fn memory_usage(&self) -> usize {
        self.total_memory_bytes.load(Ordering::Relaxed) as usize
    }

    /// Direct hash lookup. Used by `/api/prepared/text/{hash}` to fetch
    /// one statement without paying for a `get_entries()` clone of every
    /// row in every pool — the prior implementation walked all entries
    /// linearly per pool and allocated a Vec along the way for what was
    /// always a single-row answer.
    pub fn lookup_by_hash(&self, hash: u64) -> Option<(Arc<Parse>, CacheEntryKind)> {
        self.cache
            .get(&hash)
            .map(|entry| (entry.parse.clone(), entry.kind()))
    }

    /// Returns all entries with stats. Tuple is
    /// `(hash, parse, count_used, kind, hit_count, miss_count)`.
    pub fn get_entries(&self) -> Vec<(u64, Arc<Parse>, u64, CacheEntryKind, u64, u64)> {
        self.cache
            .iter()
            .map(|entry| {
                (
                    *entry.key(),
                    entry.parse.clone(),
                    entry.count_used,
                    entry.kind(),
                    entry.hit_count.load(Ordering::Relaxed),
                    entry.miss_count.load(Ordering::Relaxed),
                )
            })
            .collect()
    }

    /// Atomically increments the hit counter on the entry for `hash`.
    /// Silently no-ops when the entry was evicted or never inserted —
    /// keeps the hot path lock-free.
    pub fn record_hit(&self, hash: u64) {
        if let Some(entry) = self.cache.get(&hash) {
            entry.hit_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Same as `record_hit`, but for misses.
    pub fn record_miss(&self, hash: u64) {
        if let Some(entry) = self.cache.get(&hash) {
            entry.miss_count.fetch_add(1, Ordering::Relaxed);
        }
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
                self.total_memory_bytes
                    .fetch_sub(entry_bytes(&entry.parse), Ordering::Relaxed);
                info!(
                    "Pool cache eviction: hash={:#x}, kind={}, name={}, query=\"{}\", size={}/{}",
                    key,
                    entry.kind().as_str(),
                    entry.parse.name,
                    truncate_query_for_log(entry.parse.query()),
                    self.cache.len(),
                    self.max_size,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{BufMut, BytesMut};
    use serial_test::serial;
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
                        cache.get_or_insert(&parse, hash, Some("stmt"));
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
        cache.get_or_insert(&parse, 1, Some("stmt_one"));
        let entries = cache.get_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].3, CacheEntryKind::Named);
    }

    #[test]
    fn flags_anonymous_only_on_anonymous_register() {
        let cache = PreparedStatementCache::new(8, 1);
        let parse = make_parse("", "SELECT 1");
        cache.get_or_insert(&parse, 1, None);
        let entries = cache.get_entries();
        assert_eq!(entries[0].3, CacheEntryKind::Anonymous);
    }

    #[test]
    fn flags_mixed_when_both_seen() {
        let cache = PreparedStatementCache::new(8, 1);
        let p1 = make_parse("stmt_one", "SELECT 1");
        cache.get_or_insert(&p1, 1, Some("stmt_one"));
        let p2 = make_parse("", "SELECT 1");
        cache.get_or_insert(&p2, 1, None);
        let entries = cache.get_entries();
        assert_eq!(entries[0].3, CacheEntryKind::Mixed);
    }

    #[test]
    fn lookup_by_hash_returns_none_for_unknown() {
        let cache = PreparedStatementCache::new(8, 1);
        assert!(cache.lookup_by_hash(0xdead_beef).is_none());
    }

    #[test]
    fn lookup_by_hash_returns_parse_and_kind() {
        let cache = PreparedStatementCache::new(8, 1);
        let parse = make_parse("stmt", "SELECT 1");
        cache.get_or_insert(&parse, 0xCAFE, Some("stmt"));
        let (got, kind) = cache.lookup_by_hash(0xCAFE).expect("entry must be present");
        assert_eq!(kind, CacheEntryKind::Named);
        assert_eq!(got.query(), "SELECT 1");
    }

    #[test]
    fn memory_usage_zero_when_empty() {
        let cache = PreparedStatementCache::new(8, 1);
        assert_eq!(cache.memory_usage(), 0);
    }

    #[test]
    fn memory_usage_tracks_inserts_and_eviction() {
        let cache = PreparedStatementCache::new(2, 1);
        let p1 = make_parse("a", "SELECT 1");
        let p2 = make_parse("b", "SELECT 22");
        let p3 = make_parse("c", "SELECT 333");

        cache.get_or_insert(&p1, 1, Some("a"));
        let after_one = cache.memory_usage();
        assert!(after_one > 0, "single insert must register bytes");

        cache.get_or_insert(&p2, 2, Some("b"));
        let after_two = cache.memory_usage();
        assert!(
            after_two > after_one,
            "second insert must add bytes ({after_one} -> {after_two})"
        );

        // Third insert pushes the cache past max_size, forcing one eviction.
        // The post-eviction total must equal the bytes for the two surviving
        // entries — the counter must have been decremented on remove.
        cache.get_or_insert(&p3, 3, Some("c"));
        let after_three = cache.memory_usage();
        assert_eq!(
            cache.len(),
            2,
            "max_size=2 must hold after the third insert + evict"
        );
        let walk: usize = cache
            .cache
            .iter()
            .map(|e| entry_bytes(&e.parse) as usize)
            .sum();
        assert_eq!(
            after_three, walk,
            "incremental counter must match the per-entry walk after eviction"
        );
    }

    #[test]
    fn memory_usage_unchanged_on_repeat_hit() {
        let cache = PreparedStatementCache::new(8, 1);
        let parse = make_parse("stmt", "SELECT 1");
        cache.get_or_insert(&parse, 1, Some("stmt"));
        let after_one = cache.memory_usage();
        // Second call hits the fast path — must not double-count.
        cache.get_or_insert(&parse, 1, Some("stmt"));
        assert_eq!(cache.memory_usage(), after_one);
    }

    /// A repeated hit with the same kind must not mutate the bitmask
    /// beyond the bit set at construction. The cache-line-friendly
    /// test-and-set guard relies on this invariant; verify the visible
    /// outcome (the kind) stays exactly Named, never accidentally Mixed.
    #[test]
    fn flags_set_only_when_state_actually_changes() {
        let cache = PreparedStatementCache::new(8, 1);
        let parse = make_parse("stmt_one", "SELECT 1");
        cache.get_or_insert(&parse, 1, Some("stmt_one")); // bits = FLAG_NAMED
        cache.get_or_insert(&parse, 1, Some("stmt_one")); // hit, no real state change
        let entries = cache.get_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].3, CacheEntryKind::Named);
    }

    #[test]
    #[serial(query_interner)]
    fn intern_query_named_lands_in_named_interner() {
        reset_interners_for_test();
        let arc = intern_query("select 1", 0xAA, false);
        assert!(named_entry_for_test(0xAA).is_some());
        assert!(anon_entry_for_test(0xAA).is_none());
        assert_eq!(&*arc, "select 1");
    }

    #[test]
    #[serial(query_interner)]
    fn intern_query_anonymous_lands_in_anon_interner() {
        reset_interners_for_test();
        let _arc = intern_query("select 2", 0xBB, true);
        assert!(anon_entry_for_test(0xBB).is_some());
        assert!(named_entry_for_test(0xBB).is_none());
    }

    /// Same hash routed both as named and anonymous lives in both maps with
    /// independent allocations. The dedup loss in this rare mixed case is
    /// the documented trade-off of the split refactor.
    #[test]
    #[serial(query_interner)]
    fn intern_query_same_hash_in_both_interners_independent() {
        reset_interners_for_test();
        let a_named = intern_query("select 3", 0xCC, false);
        let a_anon = intern_query("select 3", 0xCC, true);
        assert!(!Arc::ptr_eq(&a_named, &a_anon));
        assert!(named_entry_for_test(0xCC).is_some());
        assert!(anon_entry_for_test(0xCC).is_some());
    }

    /// Within a single kind, repeated intern of the same hash returns the
    /// same `Arc<str>` — the dedup property the interner exists for.
    #[test]
    #[serial(query_interner)]
    fn intern_query_returns_same_arc_for_same_hash_within_kind() {
        reset_interners_for_test();
        let a = intern_query("select 4", 0xDD, false);
        let b = intern_query("select 4", 0xDD, false);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    #[serial(query_interner)]
    fn anon_entry_tracks_last_used() {
        reset_interners_for_test();
        let _ = intern_query("select 5", 0xEE, true);
        let t0 = anon_entry_for_test(0xEE).unwrap().last_used_for_test();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let _ = intern_query("select 5", 0xEE, true);
        let t1 = anon_entry_for_test(0xEE).unwrap().last_used_for_test();
        assert!(
            t1 > t0,
            "last_used must advance on access (t0={t0}, t1={t1})"
        );
    }

    /// strong_count == 1 (only the interner holds the Arc<str>) → marked
    /// on cycle 1, evicted on cycle 2.
    #[test]
    #[serial(query_interner)]
    fn named_passive_gc_two_cycle_grace() {
        reset_interners_for_test();
        {
            let _arc = intern_query("select strangler", 0x100, false);
        }
        let s1 = gc_sweep_named();
        assert_eq!(s1.evicted, 0);
        assert!(s1.marked >= 1);
        assert!(named_entry_for_test(0x100).is_some());
        let s2 = gc_sweep_named();
        assert!(s2.evicted >= 1);
        assert!(named_entry_for_test(0x100).is_none());
    }

    /// External Arc<str> alive → strong_count > 1 → never marked.
    #[test]
    #[serial(query_interner)]
    fn named_passive_gc_keeps_referenced() {
        reset_interners_for_test();
        let _arc = intern_query("select holder", 0x101, false);
        for _ in 0..5 {
            gc_sweep_named();
        }
        assert!(named_entry_for_test(0x101).is_some());
    }

    /// Touch between marking sweep and eviction sweep must clear the mark.
    #[test]
    #[serial(query_interner)]
    fn named_passive_gc_touch_unmarks() {
        reset_interners_for_test();
        {
            let _arc = intern_query("select touched", 0x102, false);
        }
        gc_sweep_named();
        let _arc2 = intern_query("select touched", 0x102, false);
        gc_sweep_named();
        assert!(named_entry_for_test(0x102).is_some());
    }

    /// Anonymous entry idle past TTL → marked, then evicted on the next
    /// sweep that still sees it as a candidate.
    #[test]
    #[serial(query_interner)]
    fn anon_ttl_evicts_idle_with_grace() {
        reset_interners_for_test();
        let _arc = intern_query("select stale_anon", 0x103, true);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let s1 = gc_sweep_anon(10);
        assert!(s1.marked >= 1);
        assert_eq!(s1.evicted, 0);
        assert!(anon_entry_for_test(0x103).is_some());
        let s2 = gc_sweep_anon(10);
        assert!(s2.evicted >= 1);
        assert!(anon_entry_for_test(0x103).is_none());
    }

    /// Touch refreshes `last_used` so the entry is no longer a TTL
    /// candidate on the second sweep.
    #[test]
    #[serial(query_interner)]
    fn anon_ttl_touch_unmarks() {
        reset_interners_for_test();
        let _arc = intern_query("select touched_anon", 0x104, true);
        std::thread::sleep(std::time::Duration::from_millis(20));
        gc_sweep_anon(10);
        let _arc2 = intern_query("select touched_anon", 0x104, true);
        gc_sweep_anon(10);
        assert!(anon_entry_for_test(0x104).is_some());
    }

    /// TTL = u64::MAX (operator sets `anon_idle_ttl_seconds = 0`) disables
    /// time-based eviction entirely.
    #[test]
    #[serial(query_interner)]
    fn anon_ttl_disabled_keeps_everything() {
        reset_interners_for_test();
        let _arc = intern_query("select forever", 0x105, true);
        std::thread::sleep(std::time::Duration::from_millis(20));
        for _ in 0..5 {
            gc_sweep_anon(u64::MAX);
        }
        assert!(anon_entry_for_test(0x105).is_some());
    }

    #[test]
    #[serial(query_interner)]
    fn record_query_count_increments_named_entry() {
        reset_interners_for_test();
        let _ = intern_query("select 100", 0xC0FFEE, false);
        super::record_query_count(0xC0FFEE, false);
        super::record_query_count(0xC0FFEE, false);
        let snap = super::named_snapshot();
        let (_, e) = snap.iter().find(|(h, _)| *h == 0xC0FFEE).unwrap();
        assert!(e.count() >= 2);
    }

    #[test]
    fn record_query_count_no_op_on_unknown_hash() {
        // Intentionally use a hash that is not interned — must not panic.
        super::record_query_count(0xDEADC0DE, false);
        super::record_query_count(0xDEADC0DE, true);
    }

    #[test]
    fn record_hit_no_op_when_hash_absent() {
        let cache = PreparedStatementCache::new(8, 1);
        cache.record_hit(0xDEADBEEF);
        cache.record_miss(0xDEADBEEF);
        // No panic = pass; counters unobservable on absent hash.
    }

    #[test]
    fn record_hit_increments_existing_entry() {
        let cache = PreparedStatementCache::new(8, 1);
        let parse = make_parse("stmt", "SELECT 1");
        cache.get_or_insert(&parse, 0x1111, Some("stmt"));
        cache.record_hit(0x1111);
        cache.record_hit(0x1111);
        cache.record_miss(0x1111);
        let entries = cache.get_entries();
        let row = entries.iter().find(|e| e.0 == 0x1111).unwrap();
        assert_eq!(row.4, 2, "hits");
        assert_eq!(row.5, 1, "misses");
    }

    #[test]
    #[serial(query_interner)]
    fn record_query_duration_us_accumulates() {
        reset_interners_for_test();
        let _ = intern_query("select 200", 0xD00D00, false);
        super::record_query_duration_us(0xD00D00, false, 100);
        super::record_query_duration_us(0xD00D00, false, 250);
        let snap = super::named_snapshot();
        let (_, e) = snap.iter().find(|(h, _)| *h == 0xD00D00).unwrap();
        assert_eq!(e.total_duration_us(), 350);
    }

    #[test]
    fn truncate_query_for_log_keeps_short_query_intact() {
        assert_eq!(truncate_query_for_log("select 1"), "select 1");
    }

    #[test]
    fn truncate_query_for_log_collapses_newlines_to_spaces() {
        assert_eq!(
            truncate_query_for_log("select\n1\rfrom\r\nt"),
            "select 1 from  t"
        );
    }

    #[test]
    fn truncate_query_for_log_no_ellipsis_at_exact_limit() {
        let q: String = "a".repeat(LOG_QUERY_MAX_CHARS);
        let out = truncate_query_for_log(&q);
        assert_eq!(out.chars().count(), LOG_QUERY_MAX_CHARS);
        assert!(!out.ends_with("..."));
    }

    #[test]
    fn truncate_query_for_log_appends_ellipsis_past_limit() {
        let q: String = "a".repeat(LOG_QUERY_MAX_CHARS + 5);
        let out = truncate_query_for_log(&q);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), LOG_QUERY_MAX_CHARS + 3);
    }

    #[test]
    fn truncate_query_for_log_empty_input() {
        assert_eq!(truncate_query_for_log(""), "");
    }

    #[test]
    fn truncate_query_for_log_truncates_by_chars_not_bytes() {
        // Multi-byte chars: Cyrillic 'а' is 2 bytes. LOG_QUERY_MAX_CHARS
        // characters of 'а' is 2 * LOG_QUERY_MAX_CHARS bytes; truncation
        // must count chars, not bytes — otherwise a UTF-8 boundary slice
        // panics or produces invalid UTF-8.
        let q: String = "а".repeat(LOG_QUERY_MAX_CHARS + 10);
        let out = truncate_query_for_log(&q);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), LOG_QUERY_MAX_CHARS + 3);
    }

    #[test]
    fn preview_query_keeps_short_input_untouched() {
        assert_eq!(preview_query("SELECT 1\nFROM t"), "SELECT 1\nFROM t");
    }

    #[test]
    fn preview_query_caps_at_preview_max_no_ellipsis() {
        let q: String = "a".repeat(PREVIEW_QUERY_MAX_CHARS + 50);
        let out = preview_query(&q);
        assert_eq!(out.chars().count(), PREVIEW_QUERY_MAX_CHARS);
        assert!(!out.ends_with("..."));
    }

    #[test]
    fn preview_query_handles_multi_byte_chars() {
        let q: String = "ы".repeat(PREVIEW_QUERY_MAX_CHARS + 5);
        let out = preview_query(&q);
        assert_eq!(out.chars().count(), PREVIEW_QUERY_MAX_CHARS);
    }
}
