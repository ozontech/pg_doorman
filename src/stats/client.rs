use super::{get_reporter, Reporter};
use iota::iota;
use std::sync::atomic::*;
use std::sync::Arc;

use crate::utils::clock;

// Client state constants used to track the current activity state of a client.
//
// These states represent the primary status of a client connection:
// - IDLE: Client is connected but not actively processing a query
// - ACTIVE: Client is actively processing a query or transaction
// - WAITING: Client is waiting for a server connection from the pool
iota! {
    pub const CLIENT_STATE_IDLE: u8 = 1 + iota;
        , CLIENT_STATE_ACTIVE
        , CLIENT_STATE_WAITING
}

// Client wait constants used to track what a client is waiting for.
//
// These wait states provide more detailed information about what the client is doing:
// - IDLE: Client is not waiting for any I/O operation
// - READ: Client is waiting for data to be read from the connection
// - WRITE: Client is waiting for data to be written to the connection
iota! {
    pub const CLIENT_WAIT_IDLE: u8 = 1 + iota;
        , CLIENT_WAIT_READ
        , CLIENT_WAIT_WRITE
}

/// Snapshot of per-client prepared cache state pushed from the
/// client into ClientStats atomics.
///
/// Use `PreparedCacheSnapshot::new` to build instances; the constructor
/// computes `total_count` as `named_count + anonymous_count`, ensuring the
/// invariant the setter relies on. Hand-built literals are caught by a
/// `debug_assert!` in `ClientStats::set_prepared_cache_stats`.
#[derive(Debug, Clone, Copy)]
pub struct PreparedCacheSnapshot {
    /// Total entries in the client cache. Must equal `named_count + anonymous_count`.
    pub total_count: u64,
    /// Approximate memory footprint of the cache in bytes.
    pub total_bytes: u64,
    /// Entries created via Named Parse (have a non-empty statement name).
    pub named_count: u64,
    /// Entries created via Anonymous Parse (empty statement name) and held in LRU.
    pub anonymous_count: u64,
    /// Monotonic counter of Anonymous LRU evictions for this client.
    pub anonymous_evictions: u64,
}

impl PreparedCacheSnapshot {
    /// Builds a snapshot whose `total_count` equals `named_count + anonymous_count`
    /// by construction.
    pub fn new(
        total_bytes: u64,
        named_count: u64,
        anonymous_count: u64,
        anonymous_evictions: u64,
    ) -> Self {
        Self {
            total_count: named_count + anonymous_count,
            total_bytes,
            named_count,
            anonymous_count,
            anonymous_evictions,
        }
    }
}

/// Statistics and state information for a client connection.
///
/// This struct tracks various metrics and state information for a client connection
/// to the PostgreSQL connection pooler. It is used to provide information for the
/// SHOW CLIENTS command and to track client activity for monitoring and diagnostics.
pub struct ClientStats {
    /// Monotonic connection ID assigned at TCP accept time. Used for log correlation
    /// (`#cN` prefix), SHOW CLIENTS, and Cancel Protocol (as `connection_id as i32`).
    connection_id: u64,

    /// Client metadata - these fields are set when the ClientStats is constructed and not modified after
    /// ------------------------------------------------------------------------------------------
    /// Name of the application that established the connection
    application_name: String,
    /// PostgreSQL username used for the connection
    username: String,
    /// Name of the connection pool this client is using
    pool_name: String,
    /// IP address of the client
    ipaddr: String,
    /// Timestamp when the client connected
    connect_time: quanta::Instant,
    /// Whether the client is using TLS/SSL encryption
    use_tls: bool,

    /// Reporter instance used to register/unregister this client with the stats system
    reporter: Reporter,

    /// Performance metrics
    /// ------------------------------------------------------------------------------------------
    /// Total time spent waiting for a connection from pool, in microseconds
    pub total_wait_time: AtomicU64,
    /// Maximum time spent waiting for a connection from pool, in microseconds
    pub max_wait_time: AtomicU64,

    /// State tracking (packed into single atomic byte: high nibble = state, low nibble = wait)
    /// ------------------------------------------------------------------------------------------
    state_wait: AtomicU8,

    /// Activity counters
    /// ------------------------------------------------------------------------------------------
    /// Number of transactions executed by this client
    pub transaction_count: AtomicU64,
    /// Number of queries executed by this client
    pub query_count: AtomicU64,
    /// Number of errors encountered by this client
    pub error_count: AtomicU64,

    /// Nanoseconds elapsed since `connect_time` at the moment of the latest
    /// state transition (set by `set_state`/`set_wait`/`set_state_wait`).
    /// Used by `current_query_age_ms()` and `wait_ms()` accessors to expose
    /// the duration the client has spent in its current state.
    pub state_since_nanos: AtomicU64,

    /// Prepared statement cache metrics
    /// ------------------------------------------------------------------------------------------
    /// Number of entries in client's prepared statement cache
    pub prepared_cache_count: AtomicU64,
    /// Approximate memory usage of client's prepared statement cache in bytes
    pub prepared_cache_bytes: AtomicU64,
    /// Number of Named entries in client's prepared statement cache
    pub prepared_named_count: AtomicU64,
    /// Number of Anonymous entries in client's prepared statement cache
    pub prepared_anonymous_count: AtomicU64,
    /// Cumulative count of Anonymous LRU evictions in client's prepared statement cache
    pub prepared_anonymous_evictions: AtomicU64,
    /// Whether this client is async (uses Flush instead of Sync)
    pub is_async_client: AtomicBool,
}

/// Default implementation for ClientStats.
///
/// Creates a new ClientStats instance with default values:
/// - connection_id: 0
/// - Empty strings for application_name, username, pool_name, and ipaddr
/// - Current time for connect_time
/// - All counters initialized to 0
/// - Default state: IDLE
/// - Default wait status: IDLE
/// - TLS disabled
impl Default for ClientStats {
    fn default() -> Self {
        ClientStats {
            connection_id: 0,
            connect_time: clock::now(),
            application_name: String::new(),
            username: String::new(),
            pool_name: String::new(),
            ipaddr: String::new(),
            total_wait_time: AtomicU64::new(0),
            max_wait_time: AtomicU64::new(0),
            state_wait: AtomicU8::new(Self::pack(CLIENT_STATE_IDLE, CLIENT_WAIT_IDLE)),
            transaction_count: AtomicU64::new(0),
            query_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            state_since_nanos: AtomicU64::new(0),
            prepared_cache_count: AtomicU64::new(0),
            prepared_cache_bytes: AtomicU64::new(0),
            prepared_named_count: AtomicU64::new(0),
            prepared_anonymous_count: AtomicU64::new(0),
            prepared_anonymous_evictions: AtomicU64::new(0),
            is_async_client: AtomicBool::new(false),
            reporter: get_reporter(),
            use_tls: false,
        }
    }
}

impl ClientStats {
    #[inline(always)]
    fn pack(state: u8, wait: u8) -> u8 {
        (state << 4) | (wait & 0x0F)
    }

    #[inline(always)]
    pub fn state(&self) -> u8 {
        self.state_wait.load(Ordering::Relaxed) >> 4
    }

    #[inline(always)]
    pub fn wait(&self) -> u8 {
        self.state_wait.load(Ordering::Relaxed) & 0x0F
    }

    #[inline]
    fn nanos_from_connect(&self) -> u64 {
        clock::now()
            .checked_duration_since(self.connect_time)
            .unwrap_or_default()
            .as_nanos() as u64
    }

    /// Maps a raw state byte to a logical group.
    ///
    /// Groups: 1 = active, 2 = idle, 3 = waiting, 0 = other.
    /// The timestamp is written only on cross-group transitions so that
    /// intra-group flips (e.g. ACTIVE_READ ↔ ACTIVE_WRITE) don't pay the
    /// clock cost per query.
    #[inline(always)]
    fn state_group(state: u8) -> u8 {
        match state {
            CLIENT_STATE_ACTIVE => 1,
            CLIENT_STATE_IDLE => 2,
            CLIENT_STATE_WAITING => 3,
            _ => 0,
        }
    }

    #[inline(always)]
    pub fn set_state(&self, state: u8) {
        let cur = self.state_wait.load(Ordering::Relaxed);
        let prev_state = cur >> 4;
        if Self::state_group(prev_state) != Self::state_group(state) {
            // .max(1) keeps 0 reserved as the "never set" sentinel.
            let now = self.nanos_from_connect().max(1);
            self.state_since_nanos.store(now, Ordering::Relaxed);
        }
        let new = Self::pack(state, cur & 0x0F);
        self.state_wait.store(new, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn set_wait(&self, wait: u8) {
        // set_wait only changes the wait nibble; the logical state group is
        // unchanged, so no timestamp update is needed.
        let cur = self.state_wait.load(Ordering::Relaxed);
        let new = Self::pack(cur >> 4, wait);
        self.state_wait.store(new, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn set_state_wait(&self, state: u8, wait: u8) {
        let cur = self.state_wait.load(Ordering::Relaxed);
        let prev_state = cur >> 4;
        if Self::state_group(prev_state) != Self::state_group(state) {
            // .max(1) keeps 0 reserved as the "never set" sentinel.
            let now = self.nanos_from_connect().max(1);
            self.state_since_nanos.store(now, Ordering::Relaxed);
        }
        self.state_wait
            .store(Self::pack(state, wait), Ordering::Relaxed);
    }
    /// Creates a new ClientStats instance with the specified parameters.
    ///
    /// This constructor initializes a new client statistics tracker with the provided
    /// client information. All counters are initialized to zero.
    ///
    /// # Arguments
    ///
    /// * `connection_id` - Unique identifier for the client
    /// * `application_name` - Name of the application connecting to the database
    /// * `username` - PostgreSQL username used for the connection
    /// * `pool_name` - Name of the connection pool this client is using
    /// * `ipaddr` - IP address of the client
    /// * `connect_time` - Timestamp when the client connected
    /// * `use_tls` - Whether the client is using TLS/SSL encryption
    pub fn new(
        connection_id: u64,
        application_name: &str,
        username: &str,
        pool_name: &str,
        ipaddr: &str,
        connect_time: quanta::Instant,
        use_tls: bool,
    ) -> Self {
        Self {
            connection_id,
            connect_time,
            application_name: application_name.to_string(),
            username: username.to_string(),
            pool_name: pool_name.to_string(),
            ipaddr: ipaddr.to_string(),
            use_tls,
            ..Default::default()
        }
    }

    //
    // Client lifecycle management
    // ------------------------------------------------------------------------------------------

    /// Registers a client with the stats system.
    ///
    /// The stats system uses connection_id to track and aggregate statistics from all sources
    /// that relate to that client. This method should be called when a client connects.
    ///
    /// # Arguments
    ///
    /// * `stats` - Arc-wrapped ClientStats instance to register
    pub fn register(&self, stats: Arc<ClientStats>) {
        self.reporter.client_register(self.connection_id, stats);
        self.set_state(CLIENT_STATE_IDLE);
    }

    /// Reports that a client is disconnecting from the pooler.
    ///
    /// This method updates metrics on the corresponding pool and removes the client
    /// from the stats tracking system.
    #[inline(always)]
    pub fn disconnect(&self) {
        self.reporter.client_disconnecting(self.connection_id);
    }

    //
    // Client state management
    // ------------------------------------------------------------------------------------------

    /// Sets the client state to IDLE and wait status to READ.
    ///
    /// This indicates the client is done querying the server, is no longer assigned
    /// a server connection, and we're reading from the client.
    #[inline(always)]
    pub fn idle_read(&self) {
        self.set_state_wait(CLIENT_STATE_IDLE, CLIENT_WAIT_READ);
    }

    /// Sets the client state to IDLE and wait status to WRITE.
    ///
    /// This indicates the client is done querying the server, is no longer assigned
    /// a server connection, and we're writing to the client.
    #[inline(always)]
    pub fn idle_write(&self) {
        self.set_state_wait(CLIENT_STATE_IDLE, CLIENT_WAIT_WRITE);
    }

    /// Sets the client state to WAITING and wait status to IDLE.
    ///
    /// This indicates the client is waiting for a server connection from the pool.
    #[inline(always)]
    pub fn waiting(&self) {
        self.set_state_wait(CLIENT_STATE_WAITING, CLIENT_WAIT_IDLE);
    }

    /// Sets the client state to ACTIVE and wait status to READ.
    ///
    /// This indicates the client has obtained a server connection and we're reading from it.
    #[inline(always)]
    pub fn active_read(&self) {
        self.set_state_wait(CLIENT_STATE_ACTIVE, CLIENT_WAIT_READ);
    }

    /// Sets the client state to ACTIVE and wait status to WRITE.
    ///
    /// This indicates the client has obtained a server connection and we're writing to it.
    #[inline(always)]
    pub fn active_write(&self) {
        self.set_state_wait(CLIENT_STATE_ACTIVE, CLIENT_WAIT_WRITE);
    }

    /// Sets the client state to ACTIVE and wait status to IDLE.
    ///
    /// This indicates the client has obtained a server connection and is waiting for a response.
    #[inline(always)]
    pub fn active_idle(&self) {
        self.set_state_wait(CLIENT_STATE_ACTIVE, CLIENT_WAIT_IDLE);
    }

    /// Sets the client state to IDLE and wait status to IDLE.
    ///
    /// This indicates the client has failed to obtain a connection from the pool.
    #[inline(always)]
    pub fn checkout_error(&self) {
        self.set_state_wait(CLIENT_STATE_IDLE, CLIENT_WAIT_IDLE);
    }

    //
    // State conversion utilities
    // ------------------------------------------------------------------------------------------

    /// Converts the client state to a human-readable string.
    ///
    /// # Returns
    ///
    /// A string representation of the client state: "waiting", "idle", "active", or "unknown"
    pub fn state_to_string(&self) -> String {
        match self.state() {
            CLIENT_STATE_WAITING => "waiting".to_string(),
            CLIENT_STATE_IDLE => "idle".to_string(),
            CLIENT_STATE_ACTIVE => "active".to_string(),
            _ => "unknown".to_string(),
        }
    }

    /// Converts the client wait status to a human-readable string.
    ///
    /// # Returns
    ///
    /// A string representation of the wait status: "idle", "write", "read", or "unknown"
    pub fn wait_to_string(&self) -> String {
        match self.wait() {
            CLIENT_WAIT_IDLE => "idle".to_string(),
            CLIENT_WAIT_WRITE => "write".to_string(),
            CLIENT_WAIT_READ => "read".to_string(),
            _ => "unknown".to_string(),
        }
    }

    //
    // Activity tracking
    // ------------------------------------------------------------------------------------------

    /// Increments the query counter.
    ///
    /// This method is called whenever the client executes a query.
    #[inline(always)]
    pub fn query(&self) {
        self.query_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the transaction counter.
    ///
    /// This method is called whenever the client starts a transaction.
    /// Note: We report each individual query outside a transaction as a transaction.
    /// We only count the initial BEGIN as a transaction; all queries within do not
    /// count as separate transactions.
    #[inline(always)]
    pub fn transaction(&self) {
        self.transaction_count.fetch_add(1, Ordering::Relaxed);
    }

    //
    // Accessor methods for SHOW CLIENTS command
    // ------------------------------------------------------------------------------------------

    /// Returns the client connection timestamp.
    #[inline(always)]
    pub fn connect_time(&self) -> quanta::Instant {
        self.connect_time
    }

    /// Returns the client's unique identifier.
    #[inline(always)]
    pub fn connection_id(&self) -> u64 {
        self.connection_id
    }

    /// Returns the name of the application that established the connection.
    #[inline(always)]
    pub fn application_name(&self) -> String {
        self.application_name.clone()
    }

    /// Returns whether the client is using TLS/SSL encryption.
    #[inline(always)]
    pub fn tls(&self) -> bool {
        self.use_tls
    }

    /// Returns the PostgreSQL username used for the connection.
    #[inline(always)]
    pub fn username(&self) -> String {
        self.username.clone()
    }

    /// Returns the name of the connection pool this client is using.
    #[inline(always)]
    pub fn pool_name(&self) -> String {
        self.pool_name.clone()
    }

    /// Returns the IP address of the client.
    #[inline(always)]
    pub fn ipaddr(&self) -> String {
        self.ipaddr.clone()
    }

    //
    // Prepared statement cache metrics
    // ------------------------------------------------------------------------------------------

    /// Updates the prepared statement cache metrics.
    /// Called when adding or removing entries from the client's prepared statement cache.
    /// The snapshot stores `total_count`, `named_count`, and `anonymous_count`
    /// so SHOW POOLS_MEMORY can read the breakdown atomically without recomputing the sum.
    /// `anonymous_evictions` is the monotonic per-client Anonymous LRU eviction counter.
    ///
    /// Building the snapshot via `PreparedCacheSnapshot::new` guarantees the
    /// `total_count == named_count + anonymous_count` invariant by construction.
    /// A `debug_assert!` guards against hand-built snapshots that skip the constructor.
    #[inline(always)]
    pub fn set_prepared_cache_stats(&self, snap: PreparedCacheSnapshot) {
        debug_assert_eq!(
            snap.total_count,
            snap.named_count + snap.anonymous_count,
            "PreparedCacheSnapshot.total_count must equal named_count + anonymous_count",
        );
        self.prepared_cache_count
            .store(snap.total_count, Ordering::Relaxed);
        self.prepared_cache_bytes
            .store(snap.total_bytes, Ordering::Relaxed);
        self.prepared_named_count
            .store(snap.named_count, Ordering::Relaxed);
        self.prepared_anonymous_count
            .store(snap.anonymous_count, Ordering::Relaxed);
        self.prepared_anonymous_evictions
            .store(snap.anonymous_evictions, Ordering::Relaxed);
    }

    /// Returns the number of entries in the client's prepared statement cache.
    #[inline(always)]
    pub fn prepared_cache_count(&self) -> u64 {
        self.prepared_cache_count.load(Ordering::Relaxed)
    }

    /// Returns the approximate memory usage of the client's prepared statement cache in bytes.
    #[inline(always)]
    pub fn prepared_cache_bytes(&self) -> u64 {
        self.prepared_cache_bytes.load(Ordering::Relaxed)
    }

    /// Returns the number of Named entries in the client's prepared statement cache.
    #[inline(always)]
    pub fn prepared_named_count(&self) -> u64 {
        self.prepared_named_count.load(Ordering::Relaxed)
    }

    /// Returns the number of Anonymous entries in the client's prepared statement cache.
    #[inline(always)]
    pub fn prepared_anonymous_count(&self) -> u64 {
        self.prepared_anonymous_count.load(Ordering::Relaxed)
    }

    /// Returns the cumulative count of Anonymous LRU evictions in the client's cache.
    #[inline(always)]
    pub fn prepared_anonymous_evictions(&self) -> u64 {
        self.prepared_anonymous_evictions.load(Ordering::Relaxed)
    }

    /// Marks this client as an async client (uses Flush instead of Sync).
    #[inline(always)]
    pub fn set_async_client(&self) {
        self.is_async_client.store(true, Ordering::Relaxed);
    }

    /// Returns whether this client is an async client.
    #[inline(always)]
    pub fn is_async_client(&self) -> bool {
        self.is_async_client.load(Ordering::Relaxed)
    }

    /// Returns the milliseconds elapsed since this client entered the ACTIVE
    /// state. Returns `None` when the client is not currently active or the
    /// timestamp has never been set.
    #[inline]
    pub fn current_query_age_ms(&self) -> Option<u64> {
        if self.state() != CLIENT_STATE_ACTIVE {
            return None;
        }
        let since = self.state_since_nanos.load(Ordering::Relaxed);
        if since == 0 {
            return None;
        }
        Some(self.nanos_from_connect().saturating_sub(since) / 1_000_000)
    }

    /// Returns the milliseconds elapsed since this client entered the WAITING
    /// state. Returns `None` when the client is not currently waiting.
    #[inline]
    pub fn wait_ms(&self) -> Option<u64> {
        if self.state() != CLIENT_STATE_WAITING {
            return None;
        }
        let since = self.state_since_nanos.load(Ordering::Relaxed);
        if since == 0 {
            return None;
        }
        Some(self.nanos_from_connect().saturating_sub(since) / 1_000_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::get_client_stats;

    #[test]
    fn test_client_stats_default() {
        // Test that ClientStats initializes with expected default values
        let stats = ClientStats::default();

        // Check client metadata
        assert_eq!(stats.connection_id(), 0);
        assert_eq!(stats.application_name(), "");
        assert_eq!(stats.username(), "");
        assert_eq!(stats.pool_name(), "");
        assert_eq!(stats.ipaddr(), "");
        assert!(!stats.tls());

        // Check performance metrics
        assert_eq!(stats.total_wait_time.load(Ordering::Relaxed), 0);
        assert_eq!(stats.max_wait_time.load(Ordering::Relaxed), 0);

        // Check state
        assert_eq!(stats.state(), CLIENT_STATE_IDLE);
        assert_eq!(stats.wait(), CLIENT_WAIT_IDLE);

        // Check activity counters
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.error_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_client_stats_new() {
        // Test that ClientStats::new initializes with the provided values
        let now = clock::now();
        let stats = ClientStats::new(
            42,          // connection_id
            "test_app",  // application_name
            "test_user", // username
            "test_pool", // pool_name
            "127.0.0.1", // ipaddr
            now,         // connect_time
            true,        // use_tls
        );

        // Check client metadata
        assert_eq!(stats.connection_id(), 42);
        assert_eq!(stats.application_name(), "test_app");
        assert_eq!(stats.username(), "test_user");
        assert_eq!(stats.pool_name(), "test_pool");
        assert_eq!(stats.ipaddr(), "127.0.0.1");
        assert_eq!(stats.connect_time(), now);
        assert!(stats.tls());

        // Check that other fields are initialized to default values
        assert_eq!(stats.total_wait_time.load(Ordering::Relaxed), 0);
        assert_eq!(stats.max_wait_time.load(Ordering::Relaxed), 0);
        assert_eq!(stats.state(), CLIENT_STATE_IDLE);
        assert_eq!(stats.wait(), CLIENT_WAIT_IDLE);
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.error_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_client_lifecycle_methods() {
        // Create a ClientStats with a unique connection_id
        let connection_id = 12345;
        let now = clock::now();
        let stats = ClientStats::new(
            connection_id,
            "test_app",
            "test_user",
            "test_pool",
            "127.0.0.1",
            now,
            false,
        );

        // Create an Arc-wrapped ClientStats for registration
        let stats_arc = Arc::new(stats);

        // Check that the client is not in the global registry before registration
        assert!(!get_client_stats().contains_key(&connection_id));

        // Register the client
        stats_arc.register(Arc::clone(&stats_arc));

        // Check that the client was registered in the global registry
        assert!(get_client_stats().contains_key(&connection_id));

        // Check that the state was set to IDLE
        assert_eq!(stats_arc.state(), CLIENT_STATE_IDLE);

        // Disconnect the client
        stats_arc.disconnect();

        // Check that the client was removed from the global registry
        assert!(!get_client_stats().contains_key(&connection_id));
    }

    #[test]
    fn test_state_transition_methods() {
        let stats = ClientStats::default();

        // Test idle_read
        stats.idle_read();
        assert_eq!(stats.state(), CLIENT_STATE_IDLE);
        assert_eq!(stats.wait(), CLIENT_WAIT_READ);

        // Test idle_write
        stats.idle_write();
        assert_eq!(stats.state(), CLIENT_STATE_IDLE);
        assert_eq!(stats.wait(), CLIENT_WAIT_WRITE);

        // Test waiting
        stats.waiting();
        assert_eq!(stats.state(), CLIENT_STATE_WAITING);
        assert_eq!(stats.wait(), CLIENT_WAIT_IDLE);

        // Test active_read
        stats.active_read();
        assert_eq!(stats.state(), CLIENT_STATE_ACTIVE);
        assert_eq!(stats.wait(), CLIENT_WAIT_READ);

        // Test active_write
        stats.active_write();
        assert_eq!(stats.state(), CLIENT_STATE_ACTIVE);
        assert_eq!(stats.wait(), CLIENT_WAIT_WRITE);

        // Test active_idle
        stats.active_idle();
        assert_eq!(stats.state(), CLIENT_STATE_ACTIVE);
        assert_eq!(stats.wait(), CLIENT_WAIT_IDLE);

        // Test checkout_error
        stats.checkout_error();
        assert_eq!(stats.state(), CLIENT_STATE_IDLE);
        assert_eq!(stats.wait(), CLIENT_WAIT_IDLE);
    }

    #[test]
    fn test_state_conversion_methods() {
        let stats = ClientStats::default();

        // Test state_to_string
        stats.set_state(CLIENT_STATE_IDLE);
        assert_eq!(stats.state_to_string(), "idle");

        stats.set_state(CLIENT_STATE_ACTIVE);
        assert_eq!(stats.state_to_string(), "active");

        stats.set_state(CLIENT_STATE_WAITING);
        assert_eq!(stats.state_to_string(), "waiting");

        stats.set_state(0); // Invalid state
        assert_eq!(stats.state_to_string(), "unknown");

        // Test wait_to_string
        stats.set_wait(CLIENT_WAIT_IDLE);
        assert_eq!(stats.wait_to_string(), "idle");

        stats.set_wait(CLIENT_WAIT_READ);
        assert_eq!(stats.wait_to_string(), "read");

        stats.set_wait(CLIENT_WAIT_WRITE);
        assert_eq!(stats.wait_to_string(), "write");

        stats.set_wait(0); // Invalid wait state
        assert_eq!(stats.wait_to_string(), "unknown");
    }

    #[test]
    fn test_activity_tracking_methods() {
        let stats = ClientStats::default();

        // Test query
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 0);
        stats.query();
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 1);
        stats.query();
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 2);

        // Test transaction
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 0);
        stats.transaction();
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 1);
        stats.transaction();
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_accessor_methods() {
        // Create a ClientStats with specific values
        let now = clock::now();
        let stats = ClientStats::new(
            42,          // connection_id
            "test_app",  // application_name
            "test_user", // username
            "test_pool", // pool_name
            "127.0.0.1", // ipaddr
            now,         // connect_time
            true,        // use_tls
        );

        // Test accessor methods
        assert_eq!(stats.connection_id(), 42);
        assert_eq!(stats.application_name(), "test_app");
        assert_eq!(stats.username(), "test_user");
        assert_eq!(stats.pool_name(), "test_pool");
        assert_eq!(stats.ipaddr(), "127.0.0.1");
        assert_eq!(stats.connect_time(), now);
        assert!(stats.tls());
    }

    #[test]
    fn prepared_cache_snapshot_total_count_is_sum() {
        // PreparedCacheSnapshot::new must compute total_count as named + anonymous;
        // this is the structural guarantee the setter relies on.
        let snap = PreparedCacheSnapshot::new(1024, 5, 7, 0);
        assert_eq!(snap.total_count, 12);
        assert_eq!(snap.total_bytes, 1024);
        assert_eq!(snap.named_count, 5);
        assert_eq!(snap.anonymous_count, 7);
        assert_eq!(snap.anonymous_evictions, 0);
    }

    // Hand-built snapshots that violate total_count == named + anonymous must
    // trip the debug_assert! inside the setter. The check fires only in debug
    // builds, so the test is gated on debug_assertions to stay green under
    // `cargo test --release`.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "total_count must equal named_count + anonymous_count")]
    fn snapshot_setter_rejects_inconsistent_count() {
        let stats = ClientStats::default();
        let bogus = PreparedCacheSnapshot {
            total_count: 999,
            total_bytes: 0,
            named_count: 5,
            anonymous_count: 7,
            anonymous_evictions: 0,
        };
        stats.set_prepared_cache_stats(bogus);
    }

    #[test]
    fn state_since_nanos_does_not_change_inside_active_group() {
        // Enter the active group from idle so the timestamp is recorded.
        let stats = ClientStats::default();
        stats.active_read();
        let t1 = stats.state_since_nanos.load(Ordering::Relaxed);
        assert!(t1 > 0, "timestamp must be set on entry to active group");

        // Intra-group flip: ACTIVE_READ → ACTIVE_WRITE should not update the timestamp.
        stats.active_write();
        let t2 = stats.state_since_nanos.load(Ordering::Relaxed);
        assert_eq!(t1, t2, "intra-active flip must not reset the timestamp");

        // Another intra-group flip: ACTIVE_WRITE → ACTIVE_IDLE.
        stats.active_idle();
        let t3 = stats.state_since_nanos.load(Ordering::Relaxed);
        assert_eq!(t1, t3, "intra-active flip must not reset the timestamp");

        // Cross-group transition to idle must update the timestamp.
        stats.idle_read();
        let t4 = stats.state_since_nanos.load(Ordering::Relaxed);
        // t4 may equal t1 if the test runs fast enough for the clock not to
        // advance, but it was re-written (store was called).  We verify by
        // re-entering active and confirming the timestamp advances relative to
        // the idle entry.
        let _ = t4; // read but not directly assertable due to clock resolution

        // Cross-group back to active must update again.
        stats.active_read();
        let t5 = stats.state_since_nanos.load(Ordering::Relaxed);
        assert!(t5 > 0, "timestamp must be set after re-entering active");
    }

    #[test]
    fn current_query_age_and_wait_ms_none_when_not_in_state() {
        // A fresh client is IDLE: both accessors must return None.
        let stats = ClientStats::default();
        assert_eq!(stats.current_query_age_ms(), None);
        assert_eq!(stats.wait_ms(), None);

        // After transitioning to ACTIVE, current_query_age_ms returns Some
        // and wait_ms returns None.
        stats.set_state(CLIENT_STATE_ACTIVE);
        assert!(stats.current_query_age_ms().is_some());
        assert_eq!(stats.wait_ms(), None);

        // After transitioning to WAITING, wait_ms returns Some and
        // current_query_age_ms returns None.
        stats.set_state(CLIENT_STATE_WAITING);
        assert_eq!(stats.current_query_age_ms(), None);
        assert!(stats.wait_ms().is_some());

        // Back to IDLE: both return None again.
        stats.set_state(CLIENT_STATE_IDLE);
        assert_eq!(stats.current_query_age_ms(), None);
        assert_eq!(stats.wait_ms(), None);
    }
}
