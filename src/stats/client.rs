use super::{get_reporter, Reporter};
use iota::iota;
use std::sync::atomic::*;
use std::sync::Arc;
use tokio::time::Instant;

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

/// Statistics and state information for a client connection.
///
/// This struct tracks various metrics and state information for a client connection
/// to the PostgreSQL connection pooler. It is used to provide information for the
/// SHOW CLIENTS command and to track client activity for monitoring and diagnostics.
pub struct ClientStats {
    /// A random integer assigned to the client and used by stats to track the client
    client_id: i32,

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
    connect_time: Instant,
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
}

/// Default implementation for ClientStats.
///
/// Creates a new ClientStats instance with default values:
/// - client_id: 0
/// - Empty strings for application_name, username, pool_name, and ipaddr
/// - Current time for connect_time
/// - All counters initialized to 0
/// - Default state: IDLE
/// - Default wait status: IDLE
/// - TLS disabled
impl Default for ClientStats {
    fn default() -> Self {
        ClientStats {
            client_id: 0,
            connect_time: Instant::now(),
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

    #[inline(always)]
    pub fn set_state(&self, state: u8) {
        let cur = self.state_wait.load(Ordering::Relaxed);
        let wait = cur & 0x0F;
        let new = Self::pack(state, wait);
        self.state_wait.store(new, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn set_wait(&self, wait: u8) {
        let state = self.state_wait.load(Ordering::Relaxed) >> 4;
        let new = Self::pack(state, wait);
        self.state_wait.store(new, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn set_state_wait(&self, state: u8, wait: u8) {
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
    /// * `client_id` - Unique identifier for the client
    /// * `application_name` - Name of the application connecting to the database
    /// * `username` - PostgreSQL username used for the connection
    /// * `pool_name` - Name of the connection pool this client is using
    /// * `ipaddr` - IP address of the client
    /// * `connect_time` - Timestamp when the client connected
    /// * `use_tls` - Whether the client is using TLS/SSL encryption
    pub fn new(
        client_id: i32,
        application_name: &str,
        username: &str,
        pool_name: &str,
        ipaddr: &str,
        connect_time: Instant,
        use_tls: bool,
    ) -> Self {
        Self {
            client_id,
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
    /// The stats system uses client_id to track and aggregate statistics from all sources
    /// that relate to that client. This method should be called when a client connects.
    ///
    /// # Arguments
    ///
    /// * `stats` - Arc-wrapped ClientStats instance to register
    pub fn register(&self, stats: Arc<ClientStats>) {
        self.reporter.client_register(self.client_id, stats);
        self.set_state(CLIENT_STATE_IDLE);
    }

    /// Reports that a client is disconnecting from the pooler.
    ///
    /// This method updates metrics on the corresponding pool and removes the client
    /// from the stats tracking system.
    #[inline(always)]
    pub fn disconnect(&self) {
        self.reporter.client_disconnecting(self.client_id);
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
    pub fn connect_time(&self) -> Instant {
        self.connect_time
    }

    /// Returns the client's unique identifier.
    #[inline(always)]
    pub fn client_id(&self) -> i32 {
        self.client_id
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
        assert_eq!(stats.client_id(), 0);
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
        let now = Instant::now();
        let stats = ClientStats::new(
            42,          // client_id
            "test_app",  // application_name
            "test_user", // username
            "test_pool", // pool_name
            "127.0.0.1", // ipaddr
            now,         // connect_time
            true,        // use_tls
        );

        // Check client metadata
        assert_eq!(stats.client_id(), 42);
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
        // Create a ClientStats with a unique client_id
        let client_id = 12345;
        let now = Instant::now();
        let stats = ClientStats::new(
            client_id,
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
        assert!(!get_client_stats().contains_key(&client_id));

        // Register the client
        stats_arc.register(Arc::clone(&stats_arc));

        // Check that the client was registered in the global registry
        assert!(get_client_stats().contains_key(&client_id));

        // Check that the state was set to IDLE
        assert_eq!(stats_arc.state(), CLIENT_STATE_IDLE);

        // Disconnect the client
        stats_arc.disconnect();

        // Check that the client was removed from the global registry
        assert!(!get_client_stats().contains_key(&client_id));
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
        let now = Instant::now();
        let stats = ClientStats::new(
            42,          // client_id
            "test_app",  // application_name
            "test_user", // username
            "test_pool", // pool_name
            "127.0.0.1", // ipaddr
            now,         // connect_time
            true,        // use_tls
        );

        // Test accessor methods
        assert_eq!(stats.client_id(), 42);
        assert_eq!(stats.application_name(), "test_app");
        assert_eq!(stats.username(), "test_user");
        assert_eq!(stats.pool_name(), "test_pool");
        assert_eq!(stats.ipaddr(), "127.0.0.1");
        assert_eq!(stats.connect_time(), now);
        assert!(stats.tls());
    }
}
