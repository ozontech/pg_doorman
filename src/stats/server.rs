use super::AddressStats;
use super::{get_reporter, Reporter};
use crate::config::Address;
use iota::iota;
use parking_lot::RwLock;
use std::sync::atomic::*;
use std::sync::Arc;
use tokio::time::Instant;

// Server state constants used to track the current activity state of a server connection.
//
// These states represent the primary status of a server connection:
// - LOGIN: Server is in the process of establishing a connection
// - ACTIVE: Server is actively processing a query or transaction
// - IDLE: Server is connected but not actively processing a query
iota! {
    pub const SERVER_STATE_LOGIN: u8 = 1 + iota;
        , SERVER_STATE_ACTIVE
        , SERVER_STATE_IDLE
}

// Server wait constants used to track what a server connection is waiting for.
//
// These wait states provide more detailed information about what the server is doing:
// - IDLE: Server is not waiting for any I/O operation
// - READ: Server is waiting for data to be read from the connection
// - WRITE: Server is waiting for data to be written to the connection
iota! {
    pub const SERVER_WAIT_IDLE: u8 = 1 + iota;
        , SERVER_WAIT_READ
        , SERVER_WAIT_WRITE
}

/// Statistics and state information for a server connection.
///
/// This struct tracks various metrics and state information for a server connection
/// to PostgreSQL. It is used to provide information for the SHOW SERVERS command
/// and to track server activity for monitoring and diagnostics.
#[derive(Debug)]
pub struct ServerStats {
    /// A random integer assigned to the server and used by stats to track the server
    server_id: i32,
    /// PostgreSQL backend process ID
    process_id: AtomicI32,

    /// Connection context information
    /// ------------------------------------------------------------------------------------------
    /// Address configuration for this server connection
    address: Address,
    /// Timestamp when the server connection was established
    connect_time: Instant,

    /// Reporter instance used to register/unregister this server with the stats system
    reporter: Reporter,

    /// Server state and activity data
    /// ------------------------------------------------------------------------------------------
    /// Name of the application using this server connection
    pub application_name: RwLock<String>,
    /// Packed state byte: high nibble = state (LOGIN/ACTIVE/IDLE), low nibble = wait (IDLE/READ/WRITE)
    state_wait: AtomicU8,

    /// Network traffic counters
    /// ------------------------------------------------------------------------------------------
    /// Total bytes sent to the server
    pub bytes_sent: AtomicU64,
    /// Total bytes received from the server
    pub bytes_received: AtomicU64,

    /// Query and transaction counters
    /// ------------------------------------------------------------------------------------------
    /// Number of transactions processed by this server connection
    pub transaction_count: AtomicU64,
    /// Number of queries processed by this server connection
    pub query_count: AtomicU64,
    /// Number of errors encountered by this server connection
    pub error_count: AtomicU64,

    /// Prepared statement cache metrics
    /// ------------------------------------------------------------------------------------------
    /// Number of prepared statement cache hits
    pub prepared_hit_count: AtomicU64,
    /// Number of prepared statement cache misses
    pub prepared_miss_count: AtomicU64,
    /// Current size of the prepared statement cache
    pub prepared_cache_size: AtomicU64,
}

/// Default implementation for ServerStats.
///
/// Creates a new ServerStats instance with default values:
/// - server_id: 0
/// - process_id: 0
/// - Empty string for application_name
/// - Default Address
/// - Current time for connect_time
/// - Initial state: LOGIN
/// - Initial wait status: IDLE
/// - All counters initialized to 0
impl Default for ServerStats {
    fn default() -> Self {
        ServerStats {
            server_id: 0,
            process_id: AtomicI32::new(0),
            application_name: RwLock::new(String::new()),
            address: Address::default(),
            connect_time: Instant::now(),
            state_wait: AtomicU8::new(Self::pack(SERVER_STATE_LOGIN, SERVER_WAIT_IDLE)),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            transaction_count: AtomicU64::new(0),
            query_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            reporter: get_reporter(),
            prepared_hit_count: AtomicU64::new(0),
            prepared_miss_count: AtomicU64::new(0),
            prepared_cache_size: AtomicU64::new(0),
        }
    }
}

impl ServerStats {
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

    /// Creates a new ServerStats instance with the specified address and connection time.
    ///
    /// This constructor initializes a new server statistics tracker with the provided
    /// address and connection time. A random server ID is generated, and all counters
    /// are initialized to zero.
    ///
    /// # Arguments
    ///
    /// * `address` - Address configuration for this server connection
    /// * `connect_time` - Timestamp when the server connection was established
    pub fn new(address: Address, connect_time: Instant) -> Self {
        Self {
            address,
            connect_time,
            server_id: rand::random::<i32>(),
            ..Default::default()
        }
    }

    //
    // Basic accessors
    // ------------------------------------------------------------------------------------------

    /// Returns the server's unique identifier.
    pub fn server_id(&self) -> i32 {
        self.server_id
    }

    /// Returns the PostgreSQL backend process ID.
    pub fn process_id(&self) -> i32 {
        self.process_id.load(Ordering::Relaxed)
    }

    /// Updates the PostgreSQL backend process ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The new process ID to set
    pub fn update_process_id(&self, id: i32) {
        self.process_id.store(id, Ordering::Relaxed);
    }

    //
    // Server lifecycle management
    // ------------------------------------------------------------------------------------------

    /// Registers a server connection with the stats system.
    ///
    /// The stats system uses server_id to track and aggregate statistics from all sources
    /// that relate to that server. This method should be called when a server connects.
    ///
    /// # Arguments
    ///
    /// * `stats` - Arc-wrapped ServerStats instance to register
    pub fn register(&self, stats: Arc<ServerStats>) {
        self.reporter.server_register(self.server_id, stats);
        self.login();
    }

    /// Reports that a server connection is disconnecting from the pooler.
    ///
    /// This method updates metrics on the pool regarding server usage and removes
    /// the server from the stats tracking system.
    #[inline(always)]
    pub fn disconnect(&self) {
        self.reporter.server_disconnecting(self.server_id);
    }

    //
    // Server state management
    // ------------------------------------------------------------------------------------------

    /// Sets the server state to LOGIN and application name to "Undefined".
    ///
    /// This indicates the server is attempting to establish a connection to PostgreSQL.
    pub fn login(&self) {
        self.set_state(SERVER_STATE_LOGIN);
        self.set_undefined_application();
    }

    /// Sets the server state to ACTIVE and updates the application name.
    ///
    /// This indicates the server has been assigned to a client and is actively
    /// processing queries or transactions.
    ///
    /// # Arguments
    ///
    /// * `application_name` - Name of the application using this server connection
    pub fn active(&self, application_name: String) {
        self.set_state(SERVER_STATE_ACTIVE);
        self.set_application(application_name);
    }

    /// Sets the server state to IDLE and records transaction time.
    ///
    /// This indicates the server is no longer assigned to a client and is
    /// available for the next client to pick it up.
    ///
    /// # Arguments
    ///
    /// * `microseconds` - Transaction time in microseconds to record
    #[inline(always)]
    pub fn idle(&self, microseconds: u64) {
        self.address.stats.xact_time_add(microseconds);
        self.set_state(SERVER_STATE_IDLE);
    }

    /// Records transaction time and sets the server state to IDLE.
    ///
    /// This is a variant of the idle() method that emphasizes recording
    /// transaction time.
    ///
    /// # Arguments
    ///
    /// * `microseconds` - Transaction time in microseconds to record
    #[inline(always)]
    pub fn add_xact_time_and_idle(&self, microseconds: u64) {
        self.set_state(SERVER_STATE_IDLE);
        self.address.stats.xact_time_add(microseconds);
    }

    //
    // Wait state management
    // ------------------------------------------------------------------------------------------

    /// Sets the server wait status to READ.
    ///
    /// This indicates the server is waiting for data to be read from the connection.
    #[inline]
    pub fn wait_reading(&self) {
        self.set_wait(SERVER_WAIT_READ);
    }

    /// Sets the server wait status to WRITE.
    ///
    /// This indicates the server is waiting for data to be written to the connection.
    #[inline]
    pub fn wait_writing(&self) {
        self.set_wait(SERVER_WAIT_WRITE);
    }

    /// Sets the server wait status to IDLE.
    ///
    /// This indicates the server is not waiting for any I/O operation.
    #[inline]
    pub fn wait_idle(&self) {
        self.set_wait(SERVER_WAIT_IDLE);
    }

    //
    // State conversion utilities
    // ------------------------------------------------------------------------------------------

    /// Converts the server state to a human-readable string.
    ///
    /// # Returns
    ///
    /// A string representation of the server state: "active", "idle", "login", or "unknown"
    pub fn state_to_string(&self) -> String {
        match self.state() {
            SERVER_STATE_ACTIVE => "active".to_string(),
            SERVER_STATE_IDLE => "idle".to_string(),
            SERVER_STATE_LOGIN => "login".to_string(),
            _ => "unknown".to_string(),
        }
    }

    /// Converts the server wait status to a human-readable string.
    ///
    /// # Returns
    ///
    /// A string representation of the wait status: "idle", "read", "write", or "unknown"
    pub fn wait_to_string(&self) -> String {
        match self.wait() {
            SERVER_WAIT_IDLE => "idle".to_string(),
            SERVER_WAIT_READ => "read".to_string(),
            SERVER_WAIT_WRITE => "write".to_string(),
            _ => "unknown".to_string(),
        }
    }

    //
    // Application name management
    // ------------------------------------------------------------------------------------------

    /// Sets the application name for this server connection.
    ///
    /// # Arguments
    ///
    /// * `name` - The application name to set
    fn set_application(&self, name: String) {
        let mut application_name = self.application_name.write();
        *application_name = name;
    }

    /// Sets the application name to "Undefined".
    ///
    /// This is typically used when the server is in the login state.
    #[inline(always)]
    fn set_undefined_application(&self) {
        self.set_application(String::from("Undefined"))
    }

    //
    // Statistics access and management
    // ------------------------------------------------------------------------------------------

    /// Returns a reference to the address statistics for this server.
    ///
    /// # Returns
    ///
    /// An Arc-wrapped AddressStats instance
    pub fn address_stats(&self) -> Arc<AddressStats> {
        self.address.stats.clone()
    }

    /// Checks if the address statistics averages have been updated.
    ///
    /// # Returns
    ///
    /// True if the averages have been updated, false otherwise
    pub fn check_address_stat_average_is_updated_status(&self) -> bool {
        self.address.stats.averages_updated.load(Ordering::Relaxed)
    }

    /// Sets the address statistics averages updated status.
    ///
    /// # Arguments
    ///
    /// * `is_checked` - The new status to set
    pub fn set_address_stat_average_is_updated_status(&self, is_checked: bool) {
        self.address
            .stats
            .averages_updated
            .store(is_checked, Ordering::Relaxed);
    }

    //
    // Activity tracking
    // ------------------------------------------------------------------------------------------

    /// Records checkout time and updates the application name.
    ///
    /// This method is called when a server connection is checked out from the pool.
    ///
    /// # Arguments
    ///
    /// * `microseconds` - Checkout time in microseconds
    /// * `application_name` - Name of the application using this server connection
    #[inline(always)]
    pub fn checkout_time(&self, microseconds: u64, application_name: String) {
        // Update server stats and address aggregation stats
        self.set_application(application_name);
        self.address.stats.wait_time_add(microseconds);
    }

    /// Records a query execution and updates related statistics.
    ///
    /// # Arguments
    ///
    /// * `microseconds` - Query execution time in microseconds
    /// * `application_name` - Name of the application executing the query
    #[inline(always)]
    pub fn query(&self, microseconds: u64, application_name: &str) {
        self.set_application(application_name.to_string());
        self.address.stats.query_count_add();
        self.address.stats.query_time_add_microseconds(microseconds);
        self.query_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a transaction execution and updates related statistics.
    ///
    /// Note: We report each individual query outside a transaction as a transaction.
    /// We only count the initial BEGIN as a transaction; all queries within do not
    /// count as separate transactions.
    ///
    /// # Arguments
    ///
    /// * `application_name` - Name of the application executing the transaction
    #[inline(always)]
    pub fn transaction(&self, application_name: &str) {
        self.set_application(application_name.to_string());
        self.transaction_count.fetch_add(1, Ordering::Relaxed);
        self.address.stats.xact_count_add();
    }

    /// Records data sent to the server and updates related statistics.
    ///
    /// # Arguments
    ///
    /// * `amount_bytes` - Number of bytes sent
    #[inline(always)]
    pub fn data_sent(&self, amount_bytes: usize) {
        self.bytes_sent
            .fetch_add(amount_bytes as u64, Ordering::Relaxed);
        self.address.stats.bytes_sent_add(amount_bytes as u64);
    }

    /// Records data received from the server and updates related statistics.
    ///
    /// # Arguments
    ///
    /// * `amount_bytes` - Number of bytes received
    #[inline(always)]
    pub fn data_received(&self, amount_bytes: usize) {
        self.bytes_received
            .fetch_add(amount_bytes as u64, Ordering::Relaxed);
        self.address.stats.bytes_received_add(amount_bytes as u64);
    }

    //
    // Prepared statement cache metrics
    // ------------------------------------------------------------------------------------------

    /// Records a prepared statement cache hit.
    ///
    /// This is called when a prepared statement already exists on the server.
    #[inline(always)]
    pub fn prepared_cache_hit(&self) {
        self.prepared_hit_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a prepared statement cache miss.
    ///
    /// This is called when a prepared statement does not exist on the server yet.
    #[inline(always)]
    pub fn prepared_cache_miss(&self) {
        self.prepared_miss_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the prepared statement cache size counter.
    ///
    /// This is called when a new prepared statement is added to the cache.
    #[inline(always)]
    pub fn prepared_cache_add(&self) {
        self.prepared_cache_size.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements the prepared statement cache size counter.
    ///
    /// This is called when a prepared statement is removed from the cache.
    #[inline(always)]
    pub fn prepared_cache_remove(&self) {
        self.prepared_cache_size.fetch_sub(1, Ordering::Relaxed);
    }

    //
    // Accessor methods for SHOW SERVERS command
    // ------------------------------------------------------------------------------------------

    /// Returns the name of the connection pool this server is using.
    pub fn pool_name(&self) -> String {
        self.address.pool_name.clone()
    }

    /// Returns the PostgreSQL username used for the connection.
    pub fn username(&self) -> String {
        self.address.username.clone()
    }

    /// Returns the address name (host:port) for this server connection.
    pub fn address_name(&self) -> String {
        self.address.name()
    }

    /// Returns the server connection timestamp.
    pub fn connect_time(&self) -> Instant {
        self.connect_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::get_server_stats;

    #[test]
    fn test_server_stats_default() {
        // Test that ServerStats::default initializes with expected default values
        let stats = ServerStats::default();

        // Check server metadata
        assert_eq!(stats.server_id(), 0);
        assert_eq!(stats.process_id(), 0);

        // Check state
        assert_eq!(stats.state(), SERVER_STATE_LOGIN);
        assert_eq!(stats.wait(), SERVER_WAIT_IDLE);

        // Check application name
        assert_eq!(*stats.application_name.read(), "");

        // Check counters
        assert_eq!(stats.bytes_sent.load(Ordering::Relaxed), 0);
        assert_eq!(stats.bytes_received.load(Ordering::Relaxed), 0);
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.error_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.prepared_hit_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.prepared_miss_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.prepared_cache_size.load(Ordering::Relaxed), 0);

        // Check address
        assert_eq!(stats.address_name(), "pool_name-0");
        assert_eq!(stats.pool_name(), "pool_name");
        assert_eq!(stats.username(), "username");
    }

    #[test]
    fn test_server_stats_new() {
        // Create a mock address
        let address = crate::config::Address::default();
        let now = Instant::now();

        // Test that ServerStats::new initializes with the provided values
        let stats = ServerStats::new(address.clone(), now);

        // Check that server_id is random (not 0)
        assert_ne!(stats.server_id(), 0);

        // Check that connect_time is set correctly
        assert_eq!(stats.connect_time(), now);

        // Check that address is set correctly
        assert_eq!(stats.address_name(), "pool_name-0");
        assert_eq!(stats.pool_name(), "pool_name");
        assert_eq!(stats.username(), "username");

        // Check that other fields are initialized to default values
        assert_eq!(stats.process_id(), 0);
        assert_eq!(stats.state(), SERVER_STATE_LOGIN);
        assert_eq!(stats.wait(), SERVER_WAIT_IDLE);
        assert_eq!(*stats.application_name.read(), "");
        assert_eq!(stats.bytes_sent.load(Ordering::Relaxed), 0);
        assert_eq!(stats.bytes_received.load(Ordering::Relaxed), 0);
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.error_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.prepared_hit_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.prepared_miss_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.prepared_cache_size.load(Ordering::Relaxed), 0);
    }

    // Helper function to create a ServerStats for testing
    fn create_test_server_stats() -> ServerStats {
        // Create a test address
        let address = crate::config::Address {
            host: "test_host".to_string(),
            port: 5432,
            virtual_pool_id: 0,
            database: "test_db".to_string(),
            username: "test_user".to_string(),
            password: "test_password".to_string(),
            pool_name: "test_pool".to_string(),
            stats: Arc::new(AddressStats::default()),
        };

        // Create a ServerStats with a fixed server_id for testing
        let now = Instant::now();
        let stats = ServerStats::new(address, now);

        // Set a known server_id for testing
        let server_id = 42; // Use 42 to match the original tests
        ServerStats { server_id, ..stats }
    }

    #[test]
    fn test_basic_accessors() {
        let stats = create_test_server_stats();

        // Test server_id
        assert_eq!(stats.server_id(), 42);

        // Test process_id and update_process_id
        assert_eq!(stats.process_id(), 0);
        stats.update_process_id(123);
        assert_eq!(stats.process_id(), 123);
    }

    #[test]
    fn test_server_lifecycle_methods() {
        // Create a test address
        let address = Address {
            host: "test_host".to_string(),
            port: 5432,
            virtual_pool_id: 0,
            database: "test_db".to_string(),
            username: "test_user".to_string(),
            password: "test_password".to_string(),
            pool_name: "test_pool".to_string(),
            stats: Arc::new(AddressStats::default()),
        };

        // Create a ServerStats with a fixed server_id for testing
        let now = Instant::now();
        let stats = ServerStats::new(address.clone(), now);

        // Set a known server_id for testing
        let server_id = 54321;
        let stats = ServerStats { server_id, ..stats };

        // Create an Arc-wrapped ServerStats for registration
        let stats_arc = Arc::new(stats);

        // Check that the server is not in the global registry before registration
        assert!(!get_server_stats().contains_key(&server_id));

        // Register the server
        stats_arc.register(Arc::clone(&stats_arc));

        // Check that the server was registered in the global registry
        assert!(get_server_stats().contains_key(&server_id));

        // Check that the state was set to LOGIN and application name to "Undefined"
        assert_eq!(stats_arc.state(), SERVER_STATE_LOGIN);
        assert_eq!(*stats_arc.application_name.read(), "Undefined");

        // Disconnect the server
        stats_arc.disconnect();

        // Check that the server was removed from the global registry
        assert!(!get_server_stats().contains_key(&server_id));
    }

    #[test]
    fn test_state_management_methods() {
        let stats = create_test_server_stats();

        // Test login
        stats.login();
        assert_eq!(stats.state(), SERVER_STATE_LOGIN);
        assert_eq!(*stats.application_name.read(), "Undefined");

        // Test active
        stats.active("TestApp".to_string());
        assert_eq!(stats.state(), SERVER_STATE_ACTIVE);
        assert_eq!(*stats.application_name.read(), "TestApp");

        // Test idle
        stats.idle(100);
        assert_eq!(stats.state(), SERVER_STATE_IDLE);
        // Check that xact_time_add was called with 100
        assert_eq!(
            stats
                .address
                .stats
                .total
                .xact_time_microseconds
                .load(Ordering::Relaxed),
            100
        );

        // Test add_xact_time_and_idle
        stats.set_state(SERVER_STATE_ACTIVE); // Reset state
        stats.add_xact_time_and_idle(200);
        assert_eq!(stats.state(), SERVER_STATE_IDLE);
        // Check that xact_time_add was called with 200
        assert_eq!(
            stats
                .address
                .stats
                .total
                .xact_time_microseconds
                .load(Ordering::Relaxed),
            300
        ); // 100 + 200
    }

    #[test]
    fn test_wait_state_methods() {
        let stats = create_test_server_stats();

        // Test wait_reading
        stats.wait_reading();
        assert_eq!(stats.wait(), SERVER_WAIT_READ);

        // Test wait_writing
        stats.wait_writing();
        assert_eq!(stats.wait(), SERVER_WAIT_WRITE);

        // Test wait_idle
        stats.wait_idle();
        assert_eq!(stats.wait(), SERVER_WAIT_IDLE);
    }

    #[test]
    fn test_state_conversion_methods() {
        let stats = create_test_server_stats();

        // Test state_to_string
        stats.set_state(SERVER_STATE_LOGIN);
        assert_eq!(stats.state_to_string(), "login");

        stats.set_state(SERVER_STATE_ACTIVE);
        assert_eq!(stats.state_to_string(), "active");

        stats.set_state(SERVER_STATE_IDLE);
        assert_eq!(stats.state_to_string(), "idle");

        stats.set_state(0); // Invalid state
        assert_eq!(stats.state_to_string(), "unknown");

        // Test wait_to_string
        stats.set_wait(SERVER_WAIT_IDLE);
        assert_eq!(stats.wait_to_string(), "idle");

        stats.set_wait(SERVER_WAIT_READ);
        assert_eq!(stats.wait_to_string(), "read");

        stats.set_wait(SERVER_WAIT_WRITE);
        assert_eq!(stats.wait_to_string(), "write");

        stats.set_wait(0); // Invalid wait state
        assert_eq!(stats.wait_to_string(), "unknown");
    }

    #[test]
    fn test_application_name_management() {
        let stats = create_test_server_stats();

        // Test set_application (indirectly through active)
        stats.active("TestApp".to_string());
        assert_eq!(*stats.application_name.read(), "TestApp");

        // Test set_undefined_application (indirectly through login)
        stats.login();
        assert_eq!(*stats.application_name.read(), "Undefined");
    }

    #[test]
    fn test_statistics_access_and_management() {
        let stats = create_test_server_stats();

        // Test address_stats
        let address_stats = stats.address_stats();
        assert!(Arc::ptr_eq(&address_stats, &stats.address.stats));

        // Test check_address_stat_average_is_updated_status
        assert!(!stats.check_address_stat_average_is_updated_status());

        // Test set_address_stat_average_is_updated_status
        stats.set_address_stat_average_is_updated_status(true);
        assert!(stats.check_address_stat_average_is_updated_status());
    }

    #[test]
    fn test_activity_tracking_methods() {
        let stats = create_test_server_stats();

        // Test checkout_time
        stats.checkout_time(100, "TestApp".to_string());
        assert_eq!(*stats.application_name.read(), "TestApp");
        assert_eq!(
            stats.address.stats.total.wait_time.load(Ordering::Relaxed),
            100
        );

        // Test query
        stats.query(200, "QueryApp");
        assert_eq!(*stats.application_name.read(), "QueryApp");
        assert_eq!(stats.query_count.load(Ordering::Relaxed), 1);
        assert_eq!(
            stats
                .address
                .stats
                .total
                .query_count
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(
            stats
                .address
                .stats
                .total
                .query_time_microseconds
                .load(Ordering::Relaxed),
            200
        );

        // Test transaction
        stats.transaction("TransactionApp");
        assert_eq!(*stats.application_name.read(), "TransactionApp");
        assert_eq!(stats.transaction_count.load(Ordering::Relaxed), 1);
        assert_eq!(
            stats.address.stats.total.xact_count.load(Ordering::Relaxed),
            1
        );

        // Test data_sent
        stats.data_sent(300);
        assert_eq!(stats.bytes_sent.load(Ordering::Relaxed), 300);
        assert_eq!(
            stats.address.stats.total.bytes_sent.load(Ordering::Relaxed),
            300
        );

        // Test data_received
        stats.data_received(400);
        assert_eq!(stats.bytes_received.load(Ordering::Relaxed), 400);
        assert_eq!(
            stats
                .address
                .stats
                .total
                .bytes_received
                .load(Ordering::Relaxed),
            400
        );
    }

    #[test]
    fn test_prepared_statement_cache_methods() {
        let stats = create_test_server_stats();

        // Test prepared_cache_hit
        stats.prepared_cache_hit();
        assert_eq!(stats.prepared_hit_count.load(Ordering::Relaxed), 1);

        // Test prepared_cache_miss
        stats.prepared_cache_miss();
        assert_eq!(stats.prepared_miss_count.load(Ordering::Relaxed), 1);

        // Test prepared_cache_add
        stats.prepared_cache_add();
        assert_eq!(stats.prepared_cache_size.load(Ordering::Relaxed), 1);

        // Test prepared_cache_remove
        stats.prepared_cache_remove();
        assert_eq!(stats.prepared_cache_size.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_accessor_methods() {
        let stats = create_test_server_stats();

        // Test pool_name
        assert_eq!(stats.pool_name(), "test_pool");

        // Test username
        assert_eq!(stats.username(), "test_user");

        // Test address_name
        assert_eq!(stats.address_name(), "test_pool-0");

        // Test connect_time
        let connect_time = stats.connect_time();
        assert!(connect_time <= Instant::now());
    }
}
