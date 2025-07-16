/// Pool statistics and reporting for the PostgreSQL connection pooler.
///
/// This module provides functionality for collecting, aggregating, and reporting statistics
/// about connection pools. It tracks various metrics including:
///
/// - Client connection states (idle, active, waiting)
/// - Server connection states (active, idle, login)
/// - Transaction and query counts and execution times
/// - Network throughput (bytes sent/received)
/// - Wait times and error counts
/// - Performance percentiles (p50, p90, p95, p99)
///
/// The statistics are used by administrative commands like SHOW POOLS, SHOW POOLS EXTENDED,
/// and SHOW STATS to provide insights into the pooler's operation and performance.
use log::{debug, error, warn};

use crate::config::get_config;
use crate::{config::PoolMode, messages::DataType, pool::PoolIdentifierVirtual};
use std::collections::HashMap;
use std::sync::atomic::*;
use std::sync::Arc;

use crate::pool::{get_all_pools, StatsPoolIdentifier};
use crate::stats::client::{CLIENT_STATE_ACTIVE, CLIENT_STATE_IDLE, CLIENT_STATE_WAITING};
use crate::stats::percenitle::percentile_of_sorted;
use crate::stats::server::{SERVER_STATE_ACTIVE, SERVER_STATE_IDLE, SERVER_STATE_LOGIN};
use crate::stats::ClientStats;
use crate::stats::ServerStats;

#[derive(Debug, Clone)]
/// Comprehensive statistics for a PostgreSQL connection pool.
///
/// This struct tracks various metrics about a connection pool, including client and server
/// connection states, performance metrics, and aggregated statistics. It is used to provide
/// information for administrative commands like SHOW POOLS, SHOW POOLS EXTENDED, and SHOW STATS.
pub struct PoolStats {
    /// Virtual identifier for the pool (database name, username, and virtual pool ID)
    pub identifier: PoolIdentifierVirtual,

    /// Operating mode of the pool (session, transaction, statement)
    pub mode: PoolMode,

    //
    // Client connection state counters
    // ------------------------------------------------------------------------------------------
    /// Number of idle client connections
    pub cl_idle: u64,

    /// Number of active client connections (executing queries)
    pub cl_active: u64,

    /// Number of client connections waiting for a server connection
    pub cl_waiting: u64,

    /// Number of cancel requests from clients
    pub cl_cancel_req: u64,

    //
    // Server connection state counters
    // ------------------------------------------------------------------------------------------
    /// Number of active server connections (executing queries)
    pub sv_active: u64,

    /// Number of idle server connections (available for use)
    pub sv_idle: u64,

    /// Number of server connections currently in use
    pub sv_used: u64,

    /// Number of server connections in the login phase
    pub sv_login: u64,

    //
    // Performance metrics
    // ------------------------------------------------------------------------------------------
    /// Maximum wait time for a client to get a server connection (microseconds)
    pub maxwait: u64,

    /// Average number of transactions per second
    pub avg_xact_count: u64,

    /// Average number of queries per second
    pub avg_query_count: u64,

    /// Average wait time per transaction (microseconds)
    pub avg_wait_time: u64,

    /// Average wait times per virtual pool (milliseconds)
    pub avg_wait_time_vp_ms: Vec<f64>,

    /// Total bytes received from clients
    pub bytes_received: u64,

    /// Total bytes sent to clients
    pub bytes_sent: u64,

    /// Total transaction processing time (microseconds)
    pub xact_time: u64,

    /// Total query processing time (microseconds)
    pub query_time: u64,

    /// Total time clients spent waiting for server connections (microseconds)
    pub wait_time: u64,

    /// Total number of errors encountered
    pub errors: u64,

    //
    // Percentile calculation data
    // ------------------------------------------------------------------------------------------
    /// Raw query execution times for percentile calculations
    queries: Vec<u64>,

    /// Raw transaction execution times for percentile calculations
    xact: Vec<u64>,

    /// Flag indicating if percentiles have been calculated
    percentile_updated: bool,

    /// Percentile statistics for transaction execution times
    pub xact_percentile: Percentile,

    /// Percentile statistics for query execution times
    pub query_percentile: Percentile,

    //
    // Aggregated statistics for SHOW STATS command
    // ------------------------------------------------------------------------------------------
    /// Total number of transactions processed
    total_xact_count: u64,

    /// Total number of queries processed
    total_query_count: u64,

    /// Total bytes received from clients
    total_received: u64,

    /// Total bytes sent to clients
    total_sent: u64,

    /// Total transaction processing time (microseconds)
    total_xact_time_microseconds: u64,

    /// Total query processing time (microseconds)
    total_query_time_microseconds: u64,

    /// Average bytes received per second
    avg_recv: u64,

    /// Average bytes sent per second
    avg_sent: u64,

    /// Average transaction processing time (microseconds)
    avg_xact_time_microsecons: u64,

    /// Average query processing time (microseconds)
    avg_query_time_microseconds: u64,
}

#[derive(Debug, Clone)]
/// A wrapper struct that combines pool statistics with client-specific information.
///
/// This struct provides a way to associate pool statistics with client-specific
/// aggregated information. It serves as a container for `PoolStats` and can be
/// extended in the future to include additional client-specific metrics.
pub struct PoolClientStats {
    /// The underlying pool statistics
    pub pool_stats: PoolStats,
}

impl PoolClientStats {
    /// Creates a new PoolClientStats instance with the specified parameters.
    ///
    /// This constructor initializes a new pool client statistics tracker by creating
    /// a new PoolStats instance with the provided parameters.
    ///
    /// # Arguments
    ///
    /// * `identifier` - Virtual identifier for the pool
    /// * `mode` - Operating mode of the pool (session, transaction, statement)
    /// * `queries` - Vector of query execution times in microseconds
    /// * `xact` - Vector of transaction execution times in microseconds
    ///
    /// # Returns
    ///
    /// A new PoolClientStats instance
    pub fn new(
        identifier: PoolIdentifierVirtual,
        mode: PoolMode,
        queries: Vec<u64>,
        xact: Vec<u64>,
    ) -> Self {
        PoolClientStats {
            pool_stats: PoolStats::new(identifier, mode, queries, xact),
        }
    }
}

#[derive(Debug, Clone)]
/// Stores percentile statistics for performance metrics.
///
/// This struct holds various percentile values (p50, p90, p95, p99) for a set of measurements,
/// typically query or transaction execution times. These percentiles provide insights into
/// the distribution of performance metrics and help identify outliers.
pub struct Percentile {
    /// 99th percentile value - 99% of measurements are below this value
    pub p99: u64,

    /// 95th percentile value - 95% of measurements are below this value
    pub p95: u64,

    /// 90th percentile value - 90% of measurements are below this value
    pub p90: u64,

    /// 50th percentile value (median) - half of measurements are below this value
    pub p50: u64,
}

impl PoolStats {
    /// Creates a new PoolStats instance with the specified parameters.
    ///
    /// This constructor initializes a new pool statistics tracker with the provided
    /// pool identifier, mode, and performance data. All counters are initialized to zero,
    /// and the percentile statistics are initialized with default values.
    ///
    /// # Arguments
    ///
    /// * `identifier` - Virtual identifier for the pool (database name, username, virtual pool ID)
    /// * `mode` - Operating mode of the pool (session, transaction, statement)
    /// * `queries` - Vector of query execution times in microseconds for percentile calculations
    /// * `xact` - Vector of transaction execution times in microseconds for percentile calculations
    ///
    /// # Returns
    ///
    /// A new PoolStats instance with all counters initialized to zero
    pub fn new(
        identifier: PoolIdentifierVirtual,
        mode: PoolMode,
        queries: Vec<u64>,
        xact: Vec<u64>,
    ) -> Self {
        PoolStats {
            // Basic pool identification
            identifier,
            mode,

            // Client connection state counters
            cl_idle: 0,
            cl_active: 0,
            cl_waiting: 0,
            cl_cancel_req: 0,

            // Server connection state counters
            sv_active: 0,
            sv_idle: 0,
            sv_used: 0,
            sv_login: 0,

            // Performance metrics
            maxwait: 0,
            avg_query_count: 0,
            avg_xact_count: 0,
            avg_wait_time: 0,
            avg_wait_time_vp_ms: Vec::new(),
            bytes_received: 0,
            bytes_sent: 0,
            xact_time: 0,
            query_time: 0,
            wait_time: 0,
            errors: 0,

            // Percentile calculation data
            xact,
            queries,
            percentile_updated: false,
            xact_percentile: Percentile {
                p99: 0,
                p95: 0,
                p90: 0,
                p50: 0,
            },
            query_percentile: Percentile {
                p99: 0,
                p95: 0,
                p90: 0,
                p50: 0,
            },

            // Aggregated statistics for SHOW STATS command
            total_xact_count: 0,
            total_query_count: 0,
            total_received: 0,
            total_sent: 0,
            total_xact_time_microseconds: 0,
            total_query_time_microseconds: 0,
            avg_recv: 0,
            avg_sent: 0,
            avg_xact_time_microsecons: 0,
            avg_query_time_microseconds: 0,
        }
    }

    /// Constructs a lookup table of pool statistics by aggregating data from various sources.
    ///
    /// This method collects statistics from all pools, clients, and servers, and aggregates
    /// them into a comprehensive map of pool statistics. The process involves:
    ///
    /// 1. Initializing statistics for each virtual pool
    /// 2. Updating client and server state counters
    /// 3. Aggregating statistics from virtual pools into logical pools
    /// 4. Calculating percentiles for query and transaction times
    ///
    /// # Returns
    ///
    /// A HashMap mapping pool identifiers to their aggregated statistics
    pub fn construct_pool_lookup() -> HashMap<StatsPoolIdentifier, PoolStats> {
        // Initialize maps and get client/server statistics
        let mut virtual_map: HashMap<PoolIdentifierVirtual, PoolStats> = HashMap::new();
        let client_map = super::get_client_stats();
        let server_map = super::get_server_stats();

        // Initialize statistics for each virtual pool
        Self::initialize_virtual_pool_stats(&mut virtual_map);

        // Update client and server state counters
        Self::update_client_server_states(&mut virtual_map, &client_map, &server_map);

        // Aggregate statistics from virtual pools into logical pools
        let mut map = Self::aggregate_virtual_pool_stats(virtual_map);

        // Calculate percentiles for query and transaction times
        Self::calculate_percentiles(&mut map);

        map
    }

    pub fn generate_show_pools_header() -> Vec<(&'static str, DataType)> {
        vec![
            ("database", DataType::Text),
            ("user", DataType::Text),
            ("pool_mode", DataType::Text),
            ("cl_idle", DataType::Numeric),
            ("cl_active", DataType::Numeric),
            ("cl_waiting", DataType::Numeric),
            ("cl_cancel_req", DataType::Numeric),
            ("sv_active", DataType::Numeric),
            ("sv_idle", DataType::Numeric),
            ("sv_used", DataType::Numeric),
            ("sv_login", DataType::Numeric),
            ("maxwait", DataType::Numeric),
            ("maxwait_us", DataType::Numeric),
        ]
    }

    // generate_extended_header like odyssey.
    pub fn generate_show_pools_extended_header() -> Vec<(&'static str, DataType)> {
        vec![
            ("database", DataType::Text),
            ("user", DataType::Text),
            ("cl_active", DataType::Numeric),
            ("cl_waiting", DataType::Numeric),
            ("sv_active", DataType::Numeric),
            ("sv_idle", DataType::Numeric),
            ("sv_used", DataType::Numeric),
            ("sv_login", DataType::Numeric),
            ("maxwait", DataType::Numeric),
            ("maxwait_us", DataType::Numeric),
            ("pool_mode", DataType::Text),
            ("bytes_recieved", DataType::Numeric),
            ("bytes_sent", DataType::Numeric),
            ("query_0.99", DataType::Numeric),
            ("transaction_0.99", DataType::Numeric),
            ("query_0.95", DataType::Numeric),
            ("transaction_0.95", DataType::Numeric),
            ("query_0.5", DataType::Numeric),
            ("transaction_0.5", DataType::Numeric),
        ]
    }

    pub fn generate_show_pools_extended_row(&self) -> Vec<String> {
        vec![
            self.identifier.db.clone(),
            self.identifier.user.clone(),
            self.cl_active.to_string(),
            self.cl_waiting.to_string(),
            self.sv_active.to_string(),
            self.sv_idle.to_string(),
            self.sv_used.to_string(),
            self.sv_login.to_string(),
            (self.maxwait as f64 / 1_000_000f64).to_string(),
            (self.maxwait % 1_000_000).to_string(),
            self.mode.to_string(),
            self.bytes_received.to_string(),
            self.bytes_sent.to_string(),
            self.query_percentile.p99.to_string(),
            self.xact_percentile.p99.to_string(),
            self.query_percentile.p95.to_string(),
            self.xact_percentile.p95.to_string(),
            self.query_percentile.p50.to_string(),
            self.xact_percentile.p50.to_string(),
        ]
    }

    pub fn generate_show_pools_row(&self) -> Vec<String> {
        vec![
            self.identifier.db.clone(),
            self.identifier.user.clone(),
            self.mode.to_string(),
            self.cl_idle.to_string(),
            self.cl_active.to_string(),
            self.cl_waiting.to_string(),
            self.cl_cancel_req.to_string(),
            self.sv_active.to_string(),
            self.sv_idle.to_string(),
            self.sv_used.to_string(),
            self.sv_login.to_string(),
            (self.maxwait / 1_000_000).to_string(),
            (self.maxwait % 1_000_000).to_string(),
        ]
    }

    pub fn generate_show_stats_header() -> Vec<(&'static str, DataType)> {
        vec![
            ("database", DataType::Text),
            ("user", DataType::Text),
            ("total_xact_count", DataType::Numeric),
            ("total_query_count", DataType::Numeric),
            ("total_received", DataType::Numeric),
            ("total_sent", DataType::Numeric),
            ("total_xact_time", DataType::Numeric),
            ("total_query_time", DataType::Numeric),
            ("total_wait_time", DataType::Numeric),
            ("total_errors", DataType::Numeric),
            ("avg_xact_count", DataType::Numeric),
            ("avg_query_count", DataType::Numeric),
            ("avg_recv", DataType::Numeric),
            ("avg_sent", DataType::Numeric),
            ("avg_errors", DataType::Numeric),
            ("avg_xact_time", DataType::Numeric),
            ("avg_query_time", DataType::Numeric),
            ("avg_wait_time", DataType::Numeric),
        ]
    }

    pub fn generate_show_stats_row(&self) -> Vec<String> {
        vec![
            self.identifier.db.clone(),
            self.identifier.user.clone(),
            self.total_xact_count.to_string(),
            self.total_query_count.to_string(),
            self.total_received.to_string(),
            self.total_sent.to_string(),
            self.total_xact_time_microseconds.to_string(),
            self.total_query_time_microseconds.to_string(),
            self.wait_time.to_string(),
            self.errors.to_string(),
            self.avg_xact_count.to_string(),
            self.avg_query_count.to_string(),
            self.avg_recv.to_string(),
            self.avg_sent.to_string(),
            self.errors.to_string(),
            self.avg_xact_time_microsecons.to_string(),
            self.avg_query_time_microseconds.to_string(),
            self.avg_wait_time.to_string(),
        ]
    }

    /// Initializes statistics for each virtual pool by collecting data from address stats.
    ///
    /// This helper method creates a PoolStats instance for each virtual pool and populates
    /// it with statistics from the corresponding address stats. It collects query and
    /// transaction times, loads average and total statistics, and calculates wait times.
    ///
    /// # Arguments
    ///
    /// * `virtual_map` - A mutable reference to the map of virtual pool statistics
    fn initialize_virtual_pool_stats(virtual_map: &mut HashMap<PoolIdentifierVirtual, PoolStats>) {
        for (identifier, pool) in get_all_pools() {
            // Get address stats for this pool
            let address = pool.address().stats.clone();

            // Collect query execution times
            let mut queries = Vec::new();
            {
                let lock = address.query_times_us.lock();
                queries.extend(lock.iter())
            }

            // Collect transaction execution times
            let mut xact = Vec::new();
            {
                let lock = address.xact_times_us.lock();
                xact.extend(lock.iter())
            }

            // Create a new PoolStats instance for this virtual pool
            let mut current =
                PoolStats::new(identifier.clone(), pool.settings.pool_mode, queries, xact);

            // Load average statistics
            current.avg_xact_count = address.averages.xact_count.load(Ordering::Relaxed);
            current.avg_query_count = address.averages.query_count.load(Ordering::Relaxed);
            current.avg_recv = address.averages.bytes_received.load(Ordering::Relaxed);
            current.avg_sent = address.averages.bytes_sent.load(Ordering::Relaxed);
            current.avg_xact_time_microsecons = address
                .averages
                .xact_time_microseconds
                .load(Ordering::Relaxed);
            current.avg_query_time_microseconds = address
                .averages
                .query_time_microseconds
                .load(Ordering::Relaxed);
            current.errors = address.averages.errors.load(Ordering::Relaxed);

            // Load total statistics
            current.bytes_received = address.total.bytes_received.load(Ordering::Relaxed);
            current.bytes_sent = address.total.bytes_sent.load(Ordering::Relaxed);
            current.xact_time = address.total.xact_time_microseconds.load(Ordering::Relaxed);
            current.query_time = address
                .total
                .query_time_microseconds
                .load(Ordering::Relaxed);
            current.wait_time = address.total.wait_time.load(Ordering::Relaxed);

            // Load statistics for SHOW STATS command
            current.total_xact_count = address.total.xact_count.load(Ordering::Relaxed);
            current.total_query_count = address.total.query_count.load(Ordering::Relaxed);
            current.total_received = address.total.bytes_received.load(Ordering::Relaxed);
            current.total_sent = address.total.bytes_sent.load(Ordering::Relaxed);
            current.total_xact_time_microseconds =
                address.total.xact_time_microseconds.load(Ordering::Relaxed);
            current.total_query_time_microseconds = address
                .total
                .query_time_microseconds
                .load(Ordering::Relaxed);

            // Calculate average wait time if there are transactions
            if current.avg_xact_count > 0 {
                current.avg_wait_time =
                    address.averages.wait_time.load(Ordering::Relaxed) / current.avg_xact_count;
                current
                    .avg_wait_time_vp_ms
                    .push(current.avg_wait_time as f64 / 1_000f64);
            }

            // Add the pool stats to the virtual map
            virtual_map.insert(identifier.clone(), current);
        }
    }

    /// Updates client and server state counters in the virtual pool statistics.
    ///
    /// This helper method iterates through all clients and servers and updates the
    /// corresponding state counters in the virtual pool statistics. It also updates
    /// the maximum wait time for each pool based on client wait times.
    ///
    /// # Arguments
    ///
    /// * `virtual_map` - A mutable reference to the map of virtual pool statistics
    /// * `client_map` - A reference to the map of client statistics
    /// * `server_map` - A reference to the map of server statistics
    fn update_client_server_states(
        virtual_map: &mut HashMap<PoolIdentifierVirtual, PoolStats>,
        client_map: &HashMap<i32, Arc<ClientStats>>,
        server_map: &HashMap<i32, Arc<ServerStats>>,
    ) {
        // Iterate through all virtual pools
        for virtual_pool_id in 0..get_config().general.virtual_pool_count {
            // Update client state counters
            for client in client_map.values() {
                // Try to find the virtual pool for this client
                match virtual_map.get_mut(&PoolIdentifierVirtual {
                    db: client.pool_name(),
                    user: client.username(),
                    virtual_pool_id,
                }) {
                    Some(pool_stats) => {
                        // Update client state counter based on client state
                        match client.state.load(Ordering::Relaxed) {
                            CLIENT_STATE_ACTIVE => pool_stats.cl_active += 1,
                            CLIENT_STATE_IDLE => pool_stats.cl_idle += 1,
                            CLIENT_STATE_WAITING => pool_stats.cl_waiting += 1,
                            _ => error!("unknown client state"),
                        };

                        // Update maximum wait time
                        let max_wait = client.max_wait_time.load(Ordering::Relaxed);
                        pool_stats.maxwait = std::cmp::max(pool_stats.maxwait, max_wait);
                    }
                    None => debug!("Client from an obsolete pool"),
                }
            }

            // Update server state counters
            for server in server_map.values() {
                // Try to find the virtual pool for this server
                match virtual_map.get_mut(&PoolIdentifierVirtual {
                    db: server.pool_name(),
                    user: server.username(),
                    virtual_pool_id,
                }) {
                    Some(pool_stats) => {
                        // Update server state counter based on server state
                        match server.state.load(Ordering::Relaxed) {
                            SERVER_STATE_ACTIVE => pool_stats.sv_active += 1,
                            SERVER_STATE_IDLE => pool_stats.sv_idle += 1,
                            SERVER_STATE_LOGIN => pool_stats.sv_login += 1,
                            _ => error!("unknown server state"),
                        }
                    }
                    None => warn!("Server from an obsolete pool"),
                }
            }
        }
    }

    /// Aggregates statistics from virtual pools into logical pools.
    ///
    /// This helper method combines statistics from virtual pools with the same database
    /// and username into a single logical pool. It aggregates various metrics like
    /// transaction and query counts, bytes sent/received, and wait times.
    ///
    /// # Arguments
    ///
    /// * `virtual_map` - A map of virtual pool statistics
    ///
    /// # Returns
    ///
    /// A HashMap mapping logical pool identifiers to their aggregated statistics
    fn aggregate_virtual_pool_stats(
        virtual_map: HashMap<PoolIdentifierVirtual, PoolStats>,
    ) -> HashMap<StatsPoolIdentifier, PoolStats> {
        // Create a new map for logical pool statistics
        let mut map: HashMap<StatsPoolIdentifier, PoolStats> = HashMap::new();

        // Iterate through all virtual pool statistics
        for (id, virtual_pool_stat) in virtual_map {
            // Create a logical pool identifier (without virtual_pool_id)
            let db_name = id.db.clone();
            let user_name = id.user.clone();
            let stats_pool_id = StatsPoolIdentifier {
                db: db_name.clone(),
                user: user_name.clone(),
            };

            // Clone query and transaction times for potential use
            let queries = virtual_pool_stat.queries.clone();
            let xact = virtual_pool_stat.xact.clone();

            // Check if this logical pool already exists in the map
            let mut exists_in_map = true;

            // If the logical pool exists, update its statistics
            match map.get_mut(&stats_pool_id) {
                Some(current) => {
                    // Combine query and transaction time histories
                    current.xact.extend(xact.clone());
                    current.queries.extend(queries.clone());

                    // Aggregate average counters
                    current.avg_query_count += virtual_pool_stat.avg_query_count;
                    current.avg_xact_count += virtual_pool_stat.avg_xact_count;

                    // Aggregate throughput statistics
                    current.bytes_received += virtual_pool_stat.bytes_received;
                    current.bytes_sent += virtual_pool_stat.bytes_sent;

                    // Aggregate time statistics
                    current.xact_time += virtual_pool_stat.xact_time;
                    current.query_time += virtual_pool_stat.query_time;
                    current.wait_time += virtual_pool_stat.wait_time;

                    // Aggregate error count
                    current.errors += virtual_pool_stat.errors;

                    // Calculate average wait time if there are transactions
                    if virtual_pool_stat.avg_xact_count > 0 {
                        // Simple average of wait times
                        current.avg_wait_time =
                            (current.avg_wait_time + virtual_pool_stat.avg_wait_time) / 2;

                        // Store wait time in milliseconds for reporting
                        current
                            .avg_wait_time_vp_ms
                            .push(virtual_pool_stat.avg_wait_time as f64 / 1_000f64);
                    }

                    // Aggregate statistics for SHOW STATS command
                    current.total_xact_count += virtual_pool_stat.total_xact_count;
                    current.total_query_count += virtual_pool_stat.total_query_count;
                    current.total_received += virtual_pool_stat.total_received;
                    current.total_sent += virtual_pool_stat.total_sent;
                    current.total_xact_time_microseconds +=
                        virtual_pool_stat.total_xact_time_microseconds;
                    current.total_query_time_microseconds +=
                        virtual_pool_stat.total_query_time_microseconds;

                    // Aggregate average throughput
                    current.avg_recv += virtual_pool_stat.avg_recv;
                    current.avg_sent += virtual_pool_stat.avg_sent;

                    // Calculate weighted average for time metrics
                    // TODO: Consider transaction count weight for more accurate averages
                    current.avg_xact_time_microsecons = (current.avg_xact_time_microsecons
                        + virtual_pool_stat.avg_xact_time_microsecons)
                        / 2;
                    current.avg_query_time_microseconds = (current.avg_query_time_microseconds
                        + virtual_pool_stat.avg_query_time_microseconds)
                        / 2;
                }
                None => exists_in_map = false,
            }

            // If the logical pool doesn't exist, add it to the map
            if !exists_in_map {
                map.insert(stats_pool_id, virtual_pool_stat);
            }
        }

        map
    }

    /// Calculates percentiles for query and transaction times.
    ///
    /// This helper method iterates through all pools and calculates various percentiles
    /// (p50, p90, p95, p99) for both query and transaction execution times. These
    /// percentiles provide insights into the distribution of performance metrics.
    ///
    /// # Arguments
    ///
    /// * `map` - A mutable reference to the map of pool statistics
    fn calculate_percentiles(map: &mut HashMap<StatsPoolIdentifier, PoolStats>) {
        // Iterate through all pools
        for (id, _stat) in get_all_pools() {
            // Try to find the pool in the map
            if let Some(pool_stats) = map.get_mut(&StatsPoolIdentifier {
                db: id.db.clone(),
                user: id.user.clone(),
            }) {
                // Skip if percentiles have already been calculated
                if pool_stats.percentile_updated {
                    continue;
                }

                // Calculate query percentiles
                {
                    // Get a mutable slice of query times and sort it
                    let times = pool_stats.queries.as_mut_slice();
                    times.sort();

                    // Calculate percentiles using the percentile_of_sorted function
                    pool_stats.query_percentile.p50 = percentile_of_sorted(times, 50.0);
                    pool_stats.query_percentile.p90 = percentile_of_sorted(times, 90.0);
                    pool_stats.query_percentile.p95 = percentile_of_sorted(times, 95.0);
                    pool_stats.query_percentile.p99 = percentile_of_sorted(times, 99.0);
                }

                // Calculate transaction percentiles
                {
                    // Get a mutable slice of transaction times and sort it
                    let times = pool_stats.xact.as_mut_slice();
                    times.sort();

                    // Calculate percentiles using the percentile_of_sorted function
                    pool_stats.xact_percentile.p50 = percentile_of_sorted(times, 50.0);
                    pool_stats.xact_percentile.p90 = percentile_of_sorted(times, 90.0);
                    pool_stats.xact_percentile.p95 = percentile_of_sorted(times, 95.0);
                    pool_stats.xact_percentile.p99 = percentile_of_sorted(times, 99.0);
                }

                // Mark percentiles as updated
                pool_stats.percentile_updated = true;
            }
        }
    }
}

impl IntoIterator for PoolStats {
    type Item = (String, u64);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![
            ("cl_idle".to_string(), self.cl_idle),
            ("cl_active".to_string(), self.cl_active),
            ("cl_waiting".to_string(), self.cl_waiting),
            ("cl_cancel_req".to_string(), self.cl_cancel_req),
            ("sv_active".to_string(), self.sv_active),
            ("sv_idle".to_string(), self.sv_idle),
            ("sv_used".to_string(), self.sv_used),
            ("sv_login".to_string(), self.sv_login),
            ("maxwait".to_string(), self.maxwait / 1_000_000),
            ("maxwait_us".to_string(), self.maxwait % 1_000_000),
        ]
        .into_iter()
    }
}
