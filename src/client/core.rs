use crate::errors::Error;
/// Handle clients by pretending to be a PostgreSQL server.
use ahash::AHashMap;
use bytes::BytesMut;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::io::BufReader;

use crate::client::buffer_pool::PooledBuffer;
use crate::messages::{error_response, Parse};
use crate::pool::{get_pool, ClientServerMap, ConnectionPool};
use crate::server::ServerParameters;
use crate::stats::{ClientStats, PreparedCacheSnapshot, ServerStats};

/// Key for prepared statement cache - avoids string allocations for anonymous statements
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PreparedStatementKey {
    /// Named prepared statement (client-provided name)
    Named(String),
    /// Anonymous prepared statement (identified by hash)
    Anonymous(u64),
}

impl PreparedStatementKey {
    /// Create a key from client-given name, using hash for anonymous statements
    #[inline]
    pub fn from_name_or_hash(name: String, hash: u64) -> Self {
        if name.is_empty() {
            PreparedStatementKey::Anonymous(hash)
        } else {
            PreparedStatementKey::Named(name)
        }
    }
}

/// Borrowed view over a `PreparedStatementKey`. Used by `PreparedStatementCache::iter`
/// to yield key kinds without cloning the Named String per entry. The hot path
/// (`cache_memory_usage`, called from `update_prepared_cache_stats` after every Parse)
/// only inspects the kind and reads the borrowed `&str` — owning the key would
/// allocate a fresh String per yield.
#[derive(Debug, Clone, Copy)]
pub enum PreparedStatementKeyRef<'a> {
    /// Named prepared statement (client-provided name)
    Named(&'a str),
    /// Anonymous prepared statement (identified by hash)
    Anonymous(u64),
}

impl<'a> From<&'a PreparedStatementKey> for PreparedStatementKeyRef<'a> {
    fn from(key: &'a PreparedStatementKey) -> Self {
        match key {
            PreparedStatementKey::Named(name) => PreparedStatementKeyRef::Named(name.as_str()),
            PreparedStatementKey::Anonymous(hash) => PreparedStatementKeyRef::Anonymous(*hash),
        }
    }
}

/// Per-client prepared statement cache, split into two parts:
///   - `named`: AHashMap of client-provided statement names. Never evicted
///     by the pooler; lifecycle is owned by the client (Close, DEALLOCATE,
///     disconnect).
///   - `anonymous`: LRU keyed by query hash. Bounded by
///     `client_anonymous_prepared_cache_size`. On eviction the local
///     `Arc<Parse>` is dropped; nothing is sent to the backend.
pub struct PreparedStatementCache {
    named: AHashMap<String, CachedStatement>,
    anonymous: AnonymousCache,
}

/// Outcome of `PreparedStatementCache::put`.
///
/// `lru::LruCache::push` collapses two distinct cases into the same
/// `Some((k, v))` return: replacement of an existing key, and capacity-driven
/// eviction of a different key. Conflating them produced false positives in
/// the eviction counter when steady-state Parse traffic re-Parsed the same
/// anonymous hash. `PutOutcome` keeps the two apart so callers can bump
/// metrics only on real evictions.
pub enum PutOutcome {
    /// Key was not present; new entry inserted, no value displaced.
    Inserted,
    /// Key was already present; the old value is returned and the entry
    /// remains in the cache. Not an eviction — operator-visible counters
    /// must not increment on this outcome. The displaced value is exposed
    /// for callers that want to observe or drop it explicitly.
    #[allow(dead_code)]
    Replaced(CachedStatement),
    /// Cache was at capacity and a different key was evicted to make room.
    /// Only this outcome should bump the eviction counter. The evicted
    /// value is exposed for callers that want to inspect it before drop.
    #[allow(dead_code)]
    Evicted(CachedStatement),
}

impl std::fmt::Debug for PutOutcome {
    // Variant name is enough for diagnostics; CachedStatement carries an
    // Arc<Parse> with no Debug impl and no useful debug content for tests.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PutOutcome::Inserted => f.write_str("Inserted"),
            PutOutcome::Replaced(_) => f.write_str("Replaced(_)"),
            PutOutcome::Evicted(_) => f.write_str("Evicted(_)"),
        }
    }
}

enum AnonymousCache {
    Unlimited(AHashMap<u64, CachedStatement>),
    Limited(LruCache<u64, CachedStatement>),
}

impl PreparedStatementCache {
    /// `anon_size = 0` selects an unlimited Anonymous map (no LRU).
    pub fn new(anon_size: usize) -> Self {
        let anonymous = if anon_size > 0 {
            AnonymousCache::Limited(LruCache::new(NonZeroUsize::new(anon_size).unwrap()))
        } else {
            AnonymousCache::Unlimited(AHashMap::new())
        };
        Self {
            named: AHashMap::new(),
            anonymous,
        }
    }

    /// Returns a reference to the value corresponding to the key.
    /// Updates LRU order for Anonymous + Limited.
    #[inline]
    pub fn get(&mut self, key: &PreparedStatementKey) -> Option<&CachedStatement> {
        match key {
            PreparedStatementKey::Named(s) => self.named.get(s),
            PreparedStatementKey::Anonymous(h) => match &mut self.anonymous {
                AnonymousCache::Unlimited(m) => m.get(h),
                AnonymousCache::Limited(l) => l.get(h),
            },
        }
    }

    /// Insert into the routed map and report what happened.
    ///
    /// Named insertion always returns `Inserted` or `Replaced`. The Named
    /// map is unbounded, so capacity-driven eviction never occurs.
    /// Anonymous + Unlimited behaves the same way. Only Anonymous + Limited
    /// can return `Evicted`, and only when the LRU was full and a different
    /// key was popped to make room.
    #[must_use = "check for PutOutcome::Evicted to bump eviction metrics; otherwise discard with `let _ =`"]
    #[inline]
    pub fn put(&mut self, key: PreparedStatementKey, value: CachedStatement) -> PutOutcome {
        match key {
            PreparedStatementKey::Named(s) => match self.named.insert(s, value) {
                None => PutOutcome::Inserted,
                Some(prev) => PutOutcome::Replaced(prev),
            },
            PreparedStatementKey::Anonymous(h) => match &mut self.anonymous {
                AnonymousCache::Unlimited(m) => match m.insert(h, value) {
                    None => PutOutcome::Inserted,
                    Some(prev) => PutOutcome::Replaced(prev),
                },
                // `LruCache::push` returns `Some((k, v))` for both replacement
                // (key already present, old value returned) and eviction
                // (cache at capacity, oldest entry popped). Disambiguate by
                // probing capacity + presence beforehand so callers can tell
                // a real eviction from a steady-state replacement.
                AnonymousCache::Limited(l) => {
                    let was_at_capacity = l.len() == l.cap().get();
                    let key_existed = l.contains(&h);
                    match l.push(h, value) {
                        None => PutOutcome::Inserted,
                        Some((_, prev)) if key_existed => PutOutcome::Replaced(prev),
                        Some((_, evicted)) => {
                            debug_assert!(
                                was_at_capacity,
                                "LruCache::push returned Some without replacement \
                                 despite cache below capacity",
                            );
                            PutOutcome::Evicted(evicted)
                        }
                    }
                }
            },
        }
    }

    /// Removes a key from the cache, returning the value if it existed.
    #[inline]
    pub fn pop(&mut self, key: &PreparedStatementKey) -> Option<CachedStatement> {
        match key {
            PreparedStatementKey::Named(s) => self.named.remove(s),
            PreparedStatementKey::Anonymous(h) => match &mut self.anonymous {
                AnonymousCache::Unlimited(m) => m.remove(h),
                AnonymousCache::Limited(l) => l.pop(h),
            },
        }
    }

    /// Total number of entries across Named and Anonymous maps.
    #[inline]
    pub fn len(&self) -> usize {
        self.named_count() + self.anonymous_count()
    }

    #[allow(dead_code)]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.named.is_empty() && self.anonymous_count() == 0
    }

    #[inline]
    pub fn named_count(&self) -> usize {
        self.named.len()
    }

    #[inline]
    pub fn anonymous_count(&self) -> usize {
        match &self.anonymous {
            AnonymousCache::Unlimited(m) => m.len(),
            AnonymousCache::Limited(l) => l.len(),
        }
    }

    /// Clears both Named and Anonymous maps.
    #[inline]
    pub fn clear(&mut self) {
        self.named.clear();
        match &mut self.anonymous {
            AnonymousCache::Unlimited(m) => m.clear(),
            AnonymousCache::Limited(l) => l.clear(),
        }
    }

    /// Yields `(borrowed key, value)` for both maps. The Anonymous side
    /// produces `PreparedStatementKeyRef::Anonymous(hash)` keys, the Named
    /// side `PreparedStatementKeyRef::Named(&str)` borrowing the map's key.
    /// Order is unspecified. Note: does not affect LRU order for Anonymous + Limited.
    ///
    /// Returning a borrowed-key view avoids two allocation costs that the
    /// previous `Box<dyn Iterator<Item = (PreparedStatementKey, ...)>>`
    /// signature paid on every call: the trait-object box and a `String`
    /// clone per Named entry.
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (PreparedStatementKeyRef<'_>, &CachedStatement)> + '_ {
        let named_iter = self
            .named
            .iter()
            .map(|(k, v)| (PreparedStatementKeyRef::Named(k.as_str()), v));
        let anon_iter =
            AnonIter::new(&self.anonymous).map(|(h, v)| (PreparedStatementKeyRef::Anonymous(h), v));
        named_iter.chain(anon_iter)
    }
}

/// Unifies the two backing iterator types of `AnonymousCache`
/// (`std::collections::hash_map::Iter` and `lru::Iter`) into a single
/// concrete type so `PreparedStatementCache::iter` can return
/// `impl Iterator` without boxing. `AHashMap` derefs to `std::HashMap`,
/// so its `iter()` returns the standard library's hash_map iterator.
enum AnonIter<'a> {
    Unlimited(std::collections::hash_map::Iter<'a, u64, CachedStatement>),
    Limited(lru::Iter<'a, u64, CachedStatement>),
}

impl<'a> AnonIter<'a> {
    fn new(anon: &'a AnonymousCache) -> Self {
        match anon {
            AnonymousCache::Unlimited(m) => AnonIter::Unlimited(m.iter()),
            AnonymousCache::Limited(l) => AnonIter::Limited(l.iter()),
        }
    }
}

impl<'a> Iterator for AnonIter<'a> {
    type Item = (u64, &'a CachedStatement);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            AnonIter::Unlimited(it) => it.next().map(|(h, v)| (*h, v)),
            AnonIter::Limited(it) => it.next().map(|(h, v)| (*h, v)),
        }
    }
}

/// What response message we're waiting for to insert ParseComplete
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseCompleteTarget {
    /// Waiting for BindComplete - insert ParseComplete before it
    BindComplete,
    /// Waiting for ParameterDescription - insert ParseComplete before it (Describe flow)
    ParameterDescription,
}

/// Tracks a skipped Parse message that needs a synthetic ParseComplete response
#[derive(Debug, Clone)]
pub struct SkippedParse {
    /// The rewritten statement name (e.g., DOORMAN_5)
    pub statement_name: String,
    /// What response we're waiting for to insert ParseComplete
    pub target: ParseCompleteTarget,
    /// If true, ParseComplete should be inserted at the beginning of the response.
    /// This is set when a skipped Parse comes before a new Parse in the same batch,
    /// AND there is no corresponding Bind for this skipped Parse yet.
    pub insert_at_beginning: bool,
    /// If true, a Bind message for this statement has been processed.
    /// This prevents marking insert_at_beginning=true when a new Parse arrives,
    /// because the ParseComplete should be inserted before BindComplete, not at beginning.
    pub has_bind: bool,
}

/// Tracks response message counts across multiple chunks.
/// Replaces HashMap<char, usize> with fixed fields for better performance.
#[derive(Debug, Clone, Default)]
pub struct ResponseCounts {
    /// Count of BindComplete ('2') messages
    pub bind_complete: usize,
    /// Count of ParameterDescription ('t') messages
    pub param_desc: usize,
    /// Count of Execute (tracked via CommandComplete 'C') messages
    pub execute: usize,
    /// Count of CloseComplete ('3') messages
    pub close_complete: usize,
}

impl ResponseCounts {
    #[inline(always)]
    pub fn clear(&mut self) {
        self.bind_complete = 0;
        self.param_desc = 0;
        self.execute = 0;
        self.close_complete = 0;
    }
}

/// Tracks operations in a batch to determine correct ParseComplete insertion order
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BatchOperation {
    /// Parse was skipped (statement already on server)
    ParseSkipped { statement_name: String },
    /// Parse was sent to server
    ParseSent { statement_name: String },
    /// Describe statement (produces ParameterDescription + RowDescription)
    Describe { statement_name: String },
    /// Describe portal (produces RowDescription only)
    DescribePortal,
    /// Bind to statement
    Bind { statement_name: String },
    /// Execute portal (produces DataRow + CommandComplete)
    Execute,
    /// Close statement or portal (produces CloseComplete)
    Close,
}

/// Cached prepared statement entry.
/// For async clients, stores an optional unique name to avoid "prepared statement already exists" errors.
#[derive(Clone)]
pub struct CachedStatement {
    /// Shared Parse from pool cache (contains query text)
    pub parse: Arc<Parse>,
    /// Hash of the statement
    pub hash: u64,
    /// Unique statement name for async clients (e.g., "DOORMAN_async_12345")
    /// None for non-async clients (they use parse.name directly)
    pub async_name: Option<String>,
}

impl CachedStatement {
    /// Returns the statement name to use when communicating with the server.
    /// For async clients, returns the unique async_name; otherwise returns parse.name.
    #[inline(always)]
    pub fn server_name(&self) -> &str {
        self.async_name.as_deref().unwrap_or(&self.parse.name)
    }
}

/// State related to prepared statements handling.
/// Groups all fields needed for prepared statement caching and batch processing.
pub struct PreparedStatementState {
    /// Whether prepared statements are enabled for this client
    pub enabled: bool,

    /// Whether this client has ever used async protocol (Flush command)
    /// Once set to true, prepared statements caching is disabled for this client
    pub async_client: bool,

    /// Mapping of client named prepared statement to cached statement info
    pub cache: PreparedStatementCache,

    /// Hash of the last anonymous prepared statement (for Bind to find the corresponding Parse)
    pub last_anonymous_hash: Option<u64>,

    /// Hash of the last Bind in the current batch, plus the anonymous flag.
    /// Cleared on Sync completion. Used by /api/top/queries duration
    /// instrumentation to attribute the batch's elapsed time to a single
    /// interner entry.
    pub last_bound_for_top: Option<(u64, bool)>,

    /// Tracks skipped Parse messages that need synthetic ParseComplete responses.
    /// Each entry contains the statement name and what response we're waiting for.
    pub skipped_parses: Vec<SkippedParse>,

    /// Tracks all operations in current batch to determine correct ParseComplete insertion order.
    /// Cleared after Sync.
    pub batch_operations: Vec<BatchOperation>,

    /// Counter for Parse messages sent to server in current batch.
    /// Used to determine if skipped Parse should insert ParseComplete at beginning or before BindComplete.
    pub parses_sent_in_batch: u32,

    /// Tracks how many BindComplete/ParameterDescription messages have been processed
    /// across multiple response chunks. Used for correct ParseComplete insertion.
    pub processed_response_counts: ResponseCounts,

    /// Counter for pending CloseComplete messages to send before ReadyForQuery
    pub pending_close_complete: u32,

    /// Cumulative count of Anonymous LRU evictions in this client's cache.
    /// Surfaced via the `pg_doorman_clients_prepared_anonymous_evictions_total`
    /// Prometheus counter; a sustained non-zero rate signals that
    /// `client_anonymous_prepared_cache_size` is too small for the workload.
    pub anonymous_evictions: u64,
}

impl PreparedStatementState {
    /// Create a new PreparedStatementState. `anon_cache_size = 0` selects an
    /// unlimited Anonymous map (no LRU eviction); the Named map is always
    /// unbounded.
    pub fn new(enabled: bool, anon_cache_size: usize) -> Self {
        Self {
            enabled,
            async_client: false,
            cache: PreparedStatementCache::new(anon_cache_size),
            last_anonymous_hash: None,
            last_bound_for_top: None,
            skipped_parses: Vec::new(),
            batch_operations: Vec::new(),
            parses_sent_in_batch: 0,
            processed_response_counts: ResponseCounts::default(),
            pending_close_complete: 0,
            anonymous_evictions: 0,
        }
    }

    /// Reset batch state after Sync
    #[inline(always)]
    pub fn reset_batch(&mut self) {
        self.parses_sent_in_batch = 0;
        self.skipped_parses.clear();
        self.batch_operations.clear();
        self.processed_response_counts.clear();
    }

    /// Returns the number of Named entries in the cache.
    /// Used by SHOW POOLS_MEMORY and Prometheus to break down per-client cache.
    #[inline(always)]
    pub fn named_count(&self) -> usize {
        self.cache.named_count()
    }

    /// Returns the number of Anonymous entries in the cache.
    /// Used by SHOW POOLS_MEMORY and Prometheus to break down per-client cache.
    #[inline(always)]
    pub fn anonymous_count(&self) -> usize {
        self.cache.anonymous_count()
    }

    /// Returns the cumulative count of Anonymous LRU evictions in this cache.
    #[inline(always)]
    pub fn anonymous_evictions(&self) -> u64 {
        self.anonymous_evictions
    }

    /// Calculates approximate memory usage of the client's prepared statement cache in bytes.
    /// Iterates both Named and Anonymous maps; counts shared Parse only when the
    /// client holds the sole Arc (strong_count == 1), since otherwise the bytes
    /// are accounted for by the pool cache.
    pub fn cache_memory_usage(&self) -> usize {
        let mut total = 0;
        for (key, cached) in self.cache.iter() {
            // Key size. Named uses `s.len()` rather than the underlying
            // String capacity — exposing capacity through the iter would
            // require re-allocating an owned key per yield, defeating the
            // borrow that makes this loop allocation-free. Length is a
            // close lower bound on the actual capacity.
            total += match key {
                PreparedStatementKeyRef::Named(s) => {
                    std::mem::size_of::<PreparedStatementKey>() + s.len()
                }
                PreparedStatementKeyRef::Anonymous(_) => {
                    std::mem::size_of::<PreparedStatementKey>()
                }
            };
            // CachedStatement struct size
            total += std::mem::size_of::<CachedStatement>();
            // async_name heap allocation if present
            if let Some(ref name) = cached.async_name {
                total += name.capacity();
            }
            // For non-shared Parse (strong_count == 1), count its full size
            // This happens for async clients before the fix
            if Arc::strong_count(&cached.parse) == 1 {
                total += cached.parse.memory_usage();
            }
        }
        total
    }
}

impl Default for PreparedStatementState {
    fn default() -> Self {
        Self::new(false, 0) // 0 = unlimited
    }
}

/// The client state. One of these is created per client.
pub struct Client<S, T> {
    /// The reads are buffered (8K by default).
    pub(crate) read: BufReader<S>,

    /// We buffer the writes ourselves because we know the protocol
    /// better than a stock buffer.
    pub(crate) write: T,

    /// Internal buffer, where we place messages until we have to flush
    /// them to the backend.
    pub(crate) buffer: PooledBuffer,

    /// Address
    pub(crate) addr: std::net::SocketAddr,

    /// Cached string representation of addr — avoids per-query allocation in debug logging.
    pub(crate) addr_str: String,

    /// Reusable read buffer. Avoids heap allocation per message — clear()+reserve()
    /// reuses existing capacity. split() returns owned data to callers.
    pub(crate) read_buf: BytesMut,

    /// Monotonic connection ID assigned at TCP accept. Used in log prefix as `#cN`.
    /// Also serves as Cancel Protocol process_id (as `connection_id as i32`).
    pub(crate) connection_id: u64,

    /// The client was started with the sole reason to cancel another running query.
    pub(crate) cancel_mode: bool,

    /// In transaction mode, the connection is released after each transaction.
    /// Session mode has slightly higher throughput per client, but lower capacity.
    pub(crate) transaction_mode: bool,

    /// For query cancellation, the client is given a random secret on startup.
    pub(crate) secret_key: i32,

    /// Clients are mapped to servers while they use them. This allows a client
    /// to connect and cancel a query.
    pub(crate) client_server_map: ClientServerMap,

    /// Statistics related to this client
    pub(crate) stats: Arc<ClientStats>,

    /// Clients want to talk to admin database.
    pub(crate) admin: bool,

    /// Last server process stats we talked to.
    pub(crate) last_server_stats: Option<Arc<ServerStats>>,

    /// Connected to server
    pub(crate) connected_to_server: bool,

    /// Session mode: transaction start timestamp for per-transaction xact_time.
    /// Set when server transitions into a transaction (ReadyForQuery 'T'/'E').
    /// Consumed when transaction ends (ReadyForQuery 'I').
    pub(crate) session_xact_start: Option<quanta::Instant>,

    /// Name of the server pool for this client (This comes from the database name in the connection string)
    pub(crate) pool_name: String,

    /// Postgres user for this client (This comes from the user in the connection string)
    pub(crate) username: String,

    /// Server startup and session parameters that we're going to track
    pub(crate) server_parameters: ServerParameters,

    /// Prepared statements state (caching, batch operations, etc.)
    pub(crate) prepared: PreparedStatementState,

    pub(crate) max_memory_usage: u64,

    pub(crate) client_last_messages_in_tx: PooledBuffer,

    pub(crate) pooler_check_query_request_vec: Vec<u8>,

    /// Pending BEGIN message for deferred connection optimization.
    /// When client sends standalone "begin;", we synthesize response
    /// and defer actual BEGIN until next query arrives.
    pub(crate) client_pending_begin: Option<BytesMut>,

    /// Raw fd of the client TCP socket. Stored before tokio::io::split()
    /// because ReadHalf/WriteHalf do not expose as_raw_fd().
    /// Used for client migration during graceful reload.
    #[cfg(unix)]
    pub(crate) raw_fd: Option<std::os::unix::io::RawFd>,

    /// Raw pointer to the OpenSSL SSL object for TLS migration export.
    #[cfg(all(unix, feature = "tls-migration"))]
    pub(crate) ssl_ptr: Option<SslRawPtr>,
}

/// Wrapper around *mut c_void that implements Send+Sync.
/// Used to store the SSL* pointer for migration export.
/// SAFETY: the pointer is only used at the idle point in handle() to call
/// SSL_export_migration_state, which reads TLS state without mutation.
/// The Client task is the sole user — no concurrent access.
#[cfg(all(unix, feature = "tls-migration"))]
#[derive(Clone, Copy)]
pub struct SslRawPtr(pub(crate) *mut std::ffi::c_void);
#[cfg(all(unix, feature = "tls-migration"))]
unsafe impl Send for SslRawPtr {}
#[cfg(all(unix, feature = "tls-migration"))]
unsafe impl Sync for SslRawPtr {}

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    #[inline(always)]
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    #[inline(always)]
    pub(crate) fn disconnect_stats(&self) {
        self.stats.disconnect();
    }

    /// Updates the prepared cache statistics in ClientStats.
    /// Should be called after any modification to prepared.cache.
    #[inline(always)]
    pub(crate) fn update_prepared_cache_stats(&self) {
        self.stats
            .set_prepared_cache_stats(PreparedCacheSnapshot::new(
                self.prepared.cache_memory_usage() as u64,
                self.prepared.named_count() as u64,
                self.prepared.anonymous_count() as u64,
                self.prepared.anonymous_evictions(),
            ));
    }

    /// Retrieve connection pool, if it exists.
    /// Return an error to the client otherwise.
    pub(crate) async fn get_pool(&mut self) -> Result<ConnectionPool, Error> {
        match get_pool(&self.pool_name, &self.username) {
            Some(pool) => Ok(pool),
            None => {
                error_response(
                    &mut self.write,
                    &format!(
                        "No pool configured for database: {}, user: {}",
                        self.pool_name, self.username
                    ),
                    "3D000",
                )
                .await?;

                Err(Error::ClientError(format!(
                    "Invalid pool name {{ username: {}, pool_name: {}, application_name: {} }}",
                    self.pool_name,
                    self.username,
                    self.server_parameters.get_application_name(),
                )))
            }
        }
    }

    /// Release the server from the client: it can't cancel its queries anymore.
    #[inline(always)]
    pub fn release(&self) {
        self.client_server_map
            .remove(&(self.connection_id as i32, self.secret_key));
    }
}

impl<S, T> Drop for Client<S, T> {
    fn drop(&mut self) {
        self.client_server_map
            .remove(&(self.connection_id as i32, self.secret_key));

        // Update server stats if the client was connected to a server
        if self.connected_to_server {
            if let Some(stats) = self.last_server_stats.as_ref() {
                stats.idle(0);
            }
        }

        // Ensure client is removed from stats tracking when dropped
        // This handles cases where client disconnects unexpectedly (e.g., TCP abort)
        self.stats.disconnect();
    }
}

#[cfg(test)]
mod cache_split_tests {
    use super::*;
    use std::sync::Arc;

    fn make_cached(name: &str, query: &str) -> CachedStatement {
        let mut buf = bytes::BytesMut::new();
        use bytes::BufMut;
        buf.put_u8(b'P');
        let name_bytes = name.as_bytes();
        let query_bytes = query.as_bytes();
        let len = 4 + name_bytes.len() + 1 + query_bytes.len() + 1 + 2;
        buf.put_i32(len as i32);
        buf.put_slice(name_bytes);
        buf.put_u8(0);
        buf.put_slice(query_bytes);
        buf.put_u8(0);
        buf.put_i16(0);
        let parse: crate::messages::Parse = (&buf).try_into().unwrap();
        CachedStatement {
            parse: Arc::new(parse),
            hash: 0xdead_beef,
            async_name: None,
        }
    }

    #[test]
    fn named_entries_are_never_evicted_under_anon_pressure() {
        // Anonymous LRU size 1 — but Named must persist regardless.
        let mut cache = PreparedStatementCache::new(1);
        let named_key = PreparedStatementKey::Named("stmt_one".into());
        let _ = cache.put(named_key.clone(), make_cached("stmt_one", "SELECT 1"));

        for i in 0..5 {
            let h = i as u64;
            let _ = cache.put(
                PreparedStatementKey::Anonymous(h),
                make_cached("anon", &format!("SELECT {i}")),
            );
        }

        assert!(cache.get(&named_key).is_some(), "Named entry was evicted");
    }

    #[test]
    fn anonymous_lru_evicts_oldest_when_full() {
        let mut cache = PreparedStatementCache::new(2);
        assert!(matches!(
            cache.put(PreparedStatementKey::Anonymous(1), make_cached("a", "Q1")),
            PutOutcome::Inserted
        ));
        assert!(matches!(
            cache.put(PreparedStatementKey::Anonymous(2), make_cached("a", "Q2")),
            PutOutcome::Inserted
        ));
        let outcome = cache.put(PreparedStatementKey::Anonymous(3), make_cached("a", "Q3"));
        assert!(
            matches!(outcome, PutOutcome::Evicted(_)),
            "Capacity overflow on a fresh hash must yield PutOutcome::Evicted, got {outcome:?}",
        );
        assert!(cache.get(&PreparedStatementKey::Anonymous(1)).is_none());
        assert!(cache.get(&PreparedStatementKey::Anonymous(2)).is_some());
        assert!(cache.get(&PreparedStatementKey::Anonymous(3)).is_some());
    }

    #[test]
    fn anonymous_put_returns_replaced_for_same_hash() {
        // Re-Parsing the same anonymous hash must not signal an eviction —
        // the LRU stays at one entry, no capacity pressure, the operator
        // counter must remain at zero.
        let mut cache = PreparedStatementCache::new(4);
        assert!(matches!(
            cache.put(PreparedStatementKey::Anonymous(42), make_cached("a", "Q")),
            PutOutcome::Inserted
        ));
        let outcome = cache.put(PreparedStatementKey::Anonymous(42), make_cached("a", "Q"));
        assert!(
            matches!(outcome, PutOutcome::Replaced(_)),
            "Same-hash put must yield PutOutcome::Replaced, got {outcome:?}",
        );
        assert_eq!(cache.anonymous_count(), 1);
    }

    #[test]
    fn anonymous_lru_distinguishes_inserted_replaced_evicted() {
        // Capacity 2: walk the three outcomes in sequence.
        let mut cache = PreparedStatementCache::new(2);

        // Two distinct keys → both Inserted.
        assert!(matches!(
            cache.put(PreparedStatementKey::Anonymous(1), make_cached("a", "Q1")),
            PutOutcome::Inserted
        ));
        assert!(matches!(
            cache.put(PreparedStatementKey::Anonymous(2), make_cached("a", "Q2")),
            PutOutcome::Inserted
        ));

        // Re-Parse hash 1 at full capacity → Replaced (no eviction).
        let outcome = cache.put(PreparedStatementKey::Anonymous(1), make_cached("a", "Q1"));
        assert!(
            matches!(outcome, PutOutcome::Replaced(_)),
            "Replacement at capacity must not signal eviction, got {outcome:?}",
        );
        assert_eq!(cache.anonymous_count(), 2);

        // Third distinct hash at full capacity → Evicted; oldest (hash 2)
        // popped because hash 1 was just touched and bumped to MRU.
        let outcome = cache.put(PreparedStatementKey::Anonymous(3), make_cached("a", "Q3"));
        assert!(
            matches!(outcome, PutOutcome::Evicted(_)),
            "Distinct hash at capacity must signal eviction, got {outcome:?}",
        );
        assert!(cache.get(&PreparedStatementKey::Anonymous(2)).is_none());
        assert!(cache.get(&PreparedStatementKey::Anonymous(1)).is_some());
        assert!(cache.get(&PreparedStatementKey::Anonymous(3)).is_some());
    }

    #[test]
    fn named_put_returns_inserted_then_replaced() {
        // Named map is unbounded — capacity-driven eviction never occurs.
        // First put on a fresh name → Inserted; same name again → Replaced.
        let mut cache = PreparedStatementCache::new(0);
        let key = PreparedStatementKey::Named("stmt".into());
        let first = cache.put(key.clone(), make_cached("stmt", "Q1"));
        assert!(
            matches!(first, PutOutcome::Inserted),
            "First Named put on a fresh name must be Inserted, got {first:?}",
        );
        let second = cache.put(key.clone(), make_cached("stmt", "Q2"));
        assert!(
            matches!(second, PutOutcome::Replaced(_)),
            "Re-put on existing Named name must be Replaced, got {second:?}",
        );
        assert_eq!(cache.named_count(), 1);
    }

    #[test]
    fn anonymous_unlimited_when_size_zero() {
        let mut cache = PreparedStatementCache::new(0);
        for i in 0..1000_u64 {
            let outcome = cache.put(PreparedStatementKey::Anonymous(i), make_cached("a", "Q"));
            assert!(
                matches!(outcome, PutOutcome::Inserted),
                "Unlimited cache must not evict or replace on fresh keys, got {outcome:?}",
            );
        }
        assert_eq!(cache.anonymous_count(), 1000);
    }

    #[test]
    fn pop_routes_by_key_kind() {
        let mut cache = PreparedStatementCache::new(0);
        let _ = cache.put(
            PreparedStatementKey::Named("a".into()),
            make_cached("a", "Q"),
        );
        let _ = cache.put(PreparedStatementKey::Anonymous(1), make_cached("b", "Q"));
        assert!(cache
            .pop(&PreparedStatementKey::Named("a".into()))
            .is_some());
        assert!(cache
            .pop(&PreparedStatementKey::Named("a".into()))
            .is_none());
        assert!(cache.pop(&PreparedStatementKey::Anonymous(1)).is_some());
    }

    #[test]
    fn clear_empties_both_maps() {
        let mut cache = PreparedStatementCache::new(0);
        let _ = cache.put(
            PreparedStatementKey::Named("a".into()),
            make_cached("a", "Q"),
        );
        let _ = cache.put(PreparedStatementKey::Anonymous(1), make_cached("b", "Q"));
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.named_count(), 0);
        assert_eq!(cache.anonymous_count(), 0);
    }

    #[test]
    fn iter_yields_both_maps() {
        let mut cache = PreparedStatementCache::new(0);
        let _ = cache.put(
            PreparedStatementKey::Named("a".into()),
            make_cached("a", "Q"),
        );
        let _ = cache.put(PreparedStatementKey::Anonymous(1), make_cached("b", "Q"));
        let kinds: Vec<&str> = cache
            .iter()
            .map(|(k, _)| match k {
                PreparedStatementKeyRef::Named(_) => "named",
                PreparedStatementKeyRef::Anonymous(_) => "anon",
            })
            .collect();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&"named"));
        assert!(kinds.contains(&"anon"));
    }

    #[test]
    fn iter_borrows_named_keys_without_allocation() {
        // Regression guard for B3: iter() must yield borrowed Named keys,
        // not freshly cloned Strings. The yielded &str must point into the
        // map's owned String, so its address must match across calls and
        // not match the address of an unrelated owned copy.
        let mut cache = PreparedStatementCache::new(0);
        let name = "stmt_borrow_check".to_owned();
        let _ = cache.put(
            PreparedStatementKey::Named(name.clone()),
            make_cached("stmt", "SELECT 1"),
        );

        // Two consecutive iter() calls must hand back the same backing pointer.
        let first_ptr = cache
            .iter()
            .find_map(|(k, _)| match k {
                PreparedStatementKeyRef::Named(s) => Some(s.as_ptr()),
                _ => None,
            })
            .expect("Named entry not yielded");
        let second_ptr = cache
            .iter()
            .find_map(|(k, _)| match k {
                PreparedStatementKeyRef::Named(s) => Some(s.as_ptr()),
                _ => None,
            })
            .expect("Named entry not yielded");
        assert_eq!(
            first_ptr, second_ptr,
            "iter() must borrow the same String storage on each call"
        );
        // And it must differ from a freshly-built String — proving we are
        // not silently copying somewhere.
        assert_ne!(first_ptr, name.as_ptr());
    }

    #[test]
    fn iter_handles_fifty_named_entries() {
        // Smoke test: 50-Named-entry cache mirrors a typical ORM client.
        // Counts every yielded entry to catch any regression that would
        // truncate or panic the iterator.
        let mut cache = PreparedStatementCache::new(0);
        for i in 0..50_u32 {
            let _ = cache.put(
                PreparedStatementKey::Named(format!("stmt_{i}")),
                make_cached("stmt", "SELECT 1"),
            );
        }
        assert_eq!(cache.iter().count(), 50);

        let mut named = 0_usize;
        let mut anon = 0_usize;
        for (k, _) in cache.iter() {
            match k {
                PreparedStatementKeyRef::Named(_) => named += 1,
                PreparedStatementKeyRef::Anonymous(_) => anon += 1,
            }
        }
        assert_eq!(named, 50);
        assert_eq!(anon, 0);
    }
}
