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

use crate::{config::PoolMode, messages::DataType, pool::PoolIdentifier};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::*;
use std::sync::Arc;

use crate::pool::get_all_pools;
use crate::stats::client::{CLIENT_STATE_ACTIVE, CLIENT_STATE_IDLE, CLIENT_STATE_WAITING};
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
    /// Identifier for the pool (database name and username)
    pub identifier: PoolIdentifier,

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

    /// Percentile statistics for transaction execution times (from HDR histogram)
    pub xact_percentile: Percentile,

    /// Percentile statistics for query execution times (from HDR histogram)
    pub query_percentile: Percentile,

    //
    // Aggregated statistics for SHOW STATS command
    // ------------------------------------------------------------------------------------------
    /// Total number of transactions processed
    pub total_xact_count: u64,

    /// Total number of queries processed
    pub total_query_count: u64,

    /// Total bytes received from clients
    total_received: u64,

    /// Total bytes sent to clients
    total_sent: u64,

    /// Total transaction processing time (microseconds)
    pub total_xact_time_microseconds: u64,

    /// Total query processing time (microseconds)
    pub total_query_time_microseconds: u64,

    /// Number of entries in the pool-level prepared statement cache
    pub prepared_statements_count: u64,

    /// Approximate memory usage of the pool-level prepared statement cache in bytes
    pub prepared_statements_bytes: u64,

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
    /// Creates a new PoolStats instance with pre-calculated percentiles from HDR histograms.
    ///
    /// This constructor is optimized for use with HDR histograms where percentiles
    /// are calculated in O(1) time directly from the histogram.
    ///
    /// # Arguments
    ///
    /// * `identifier` - Identifier for the pool (database name, username)
    /// * `mode` - Operating mode of the pool (session, transaction, statement)
    /// * `query_percentile` - Pre-calculated query time percentiles
    /// * `xact_percentile` - Pre-calculated transaction time percentiles
    ///
    /// # Returns
    ///
    /// A new PoolStats instance with percentiles already set
    pub fn new_with_percentiles(
        identifier: PoolIdentifier,
        mode: PoolMode,
        query_percentile: Percentile,
        xact_percentile: Percentile,
    ) -> Self {
        PoolStats {
            identifier,
            mode,
            cl_idle: 0,
            cl_active: 0,
            cl_waiting: 0,
            cl_cancel_req: 0,
            sv_active: 0,
            sv_idle: 0,
            sv_used: 0,
            sv_login: 0,
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
            // Percentiles from HDR histogram
            xact_percentile,
            query_percentile,
            total_xact_count: 0,
            total_query_count: 0,
            total_received: 0,
            total_sent: 0,
            total_xact_time_microseconds: 0,
            total_query_time_microseconds: 0,
            prepared_statements_count: 0,
            prepared_statements_bytes: 0,
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
    pub fn construct_pool_lookup() -> HashMap<PoolIdentifier, PoolStats> {
        // Initialize maps and get client/server statistics
        let mut virtual_map: HashMap<PoolIdentifier, PoolStats> = HashMap::new();
        let client_map = super::get_client_stats();
        let server_map = super::get_server_stats();

        // Initialize statistics for each virtual pool (percentiles are calculated from HDR histograms)
        Self::initialize_pool_stats(&mut virtual_map);

        // Update client and server state counters
        Self::update_client_server_states(&mut virtual_map, &client_map, &server_map);

        // Get pool statistics (no aggregation needed since virtual pools were removed)
        Self::aggregate_pool_stats(virtual_map)
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

    pub fn generate_show_pools_extended_row(&self) -> Vec<Cow<'_, str>> {
        vec![
            Cow::Borrowed(&self.identifier.db),
            Cow::Borrowed(&self.identifier.user),
            Cow::Owned(self.cl_active.to_string()),
            Cow::Owned(self.cl_waiting.to_string()),
            Cow::Owned(self.sv_active.to_string()),
            Cow::Owned(self.sv_idle.to_string()),
            Cow::Owned(self.sv_used.to_string()),
            Cow::Owned(self.sv_login.to_string()),
            Cow::Owned((self.maxwait as f64 / 1_000_000f64).to_string()),
            Cow::Owned((self.maxwait % 1_000_000).to_string()),
            Cow::Owned(self.mode.to_string()),
            Cow::Owned(self.bytes_received.to_string()),
            Cow::Owned(self.bytes_sent.to_string()),
            Cow::Owned(self.query_percentile.p99.to_string()),
            Cow::Owned(self.xact_percentile.p99.to_string()),
            Cow::Owned(self.query_percentile.p95.to_string()),
            Cow::Owned(self.xact_percentile.p95.to_string()),
            Cow::Owned(self.query_percentile.p50.to_string()),
            Cow::Owned(self.xact_percentile.p50.to_string()),
        ]
    }

    pub fn generate_show_pools_row(&self) -> Vec<Cow<'_, str>> {
        vec![
            Cow::Borrowed(&self.identifier.db),
            Cow::Borrowed(&self.identifier.user),
            Cow::Owned(self.mode.to_string()),
            Cow::Owned(self.cl_idle.to_string()),
            Cow::Owned(self.cl_active.to_string()),
            Cow::Owned(self.cl_waiting.to_string()),
            Cow::Owned(self.cl_cancel_req.to_string()),
            Cow::Owned(self.sv_active.to_string()),
            Cow::Owned(self.sv_idle.to_string()),
            Cow::Owned(self.sv_used.to_string()),
            Cow::Owned(self.sv_login.to_string()),
            Cow::Owned((self.maxwait / 1_000_000).to_string()),
            Cow::Owned((self.maxwait % 1_000_000).to_string()),
        ]
    }

    pub fn generate_show_pools_memory_header() -> Vec<(&'static str, DataType)> {
        vec![
            ("database", DataType::Text),
            ("user", DataType::Text),
            ("prepared_statements_count", DataType::Numeric),
            ("prepared_statements_bytes", DataType::Numeric),
        ]
    }

    pub fn generate_show_pools_memory_row(&self) -> Vec<Cow<'_, str>> {
        vec![
            Cow::Borrowed(&self.identifier.db),
            Cow::Borrowed(&self.identifier.user),
            Cow::Owned(self.prepared_statements_count.to_string()),
            Cow::Owned(self.prepared_statements_bytes.to_string()),
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

    pub fn generate_show_stats_row(&self) -> Vec<Cow<'_, str>> {
        vec![
            Cow::Borrowed(&self.identifier.db),
            Cow::Borrowed(&self.identifier.user),
            Cow::Owned(self.total_xact_count.to_string()),
            Cow::Owned(self.total_query_count.to_string()),
            Cow::Owned(self.total_received.to_string()),
            Cow::Owned(self.total_sent.to_string()),
            Cow::Owned(self.total_xact_time_microseconds.to_string()),
            Cow::Owned(self.total_query_time_microseconds.to_string()),
            Cow::Owned(self.wait_time.to_string()),
            Cow::Owned(self.errors.to_string()),
            Cow::Owned(self.avg_xact_count.to_string()),
            Cow::Owned(self.avg_query_count.to_string()),
            Cow::Owned(self.avg_recv.to_string()),
            Cow::Owned(self.avg_sent.to_string()),
            Cow::Owned(self.errors.to_string()),
            Cow::Owned(self.avg_xact_time_microsecons.to_string()),
            Cow::Owned(self.avg_query_time_microseconds.to_string()),
            Cow::Owned(self.avg_wait_time.to_string()),
        ]
    }

    /// Initializes statistics for each pool by collecting data from address stats.
    ///
    /// This helper method creates a PoolStats instance for each pool and populates
    /// it with statistics from the corresponding address stats. It collects query and
    /// transaction times, loads average and total statistics, and calculates wait times.
    ///
    /// # Arguments
    ///
    /// * `map` - A mutable reference to the map of pool statistics
    fn initialize_pool_stats(map: &mut HashMap<PoolIdentifier, PoolStats>) {
        for (identifier, pool) in get_all_pools().iter() {
            // Get address stats for this pool
            let address = pool.address().stats.clone();

            // Get percentiles directly from HDR histograms (O(1) operation)
            let (query_p50, query_p90, query_p95, query_p99) = address.get_query_percentiles();
            let (xact_p50, xact_p90, xact_p95, xact_p99) = address.get_xact_percentiles();

            // Create a new PoolStats instance for this pool with pre-calculated percentiles
            let mut current = PoolStats::new_with_percentiles(
                identifier.clone(),
                pool.settings.pool_mode,
                Percentile {
                    p50: query_p50,
                    p90: query_p90,
                    p95: query_p95,
                    p99: query_p99,
                },
                Percentile {
                    p50: xact_p50,
                    p90: xact_p90,
                    p95: xact_p95,
                    p99: xact_p99,
                },
            );

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

            // Load pool-level prepared statement cache statistics
            if let Some(cache) = pool.prepared_statement_cache.as_ref() {
                current.prepared_statements_count = cache.len() as u64;
                current.prepared_statements_bytes = cache.memory_usage() as u64;
            }

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
            map.insert(identifier.clone(), current);
        }
    }

    /// Updates client and server state counters in the pool statistics.
    ///
    /// This helper method iterates through all clients and servers and updates the
    /// corresponding state counters in the pool statistics. It also updates
    /// the maximum wait time for each pool based on client wait times.
    ///
    /// # Arguments
    ///
    /// * `pool_map` - A mutable reference to the map of pool statistics
    /// * `client_map` - A reference to the map of client statistics
    /// * `server_map` - A reference to the map of server statistics
    fn update_client_server_states(
        pool_map: &mut HashMap<PoolIdentifier, PoolStats>,
        client_map: &HashMap<i32, Arc<ClientStats>>,
        server_map: &HashMap<i32, Arc<ServerStats>>,
    ) {
        // Update client state counters
        for client in client_map.values() {
            // Try to find the pool for this client
            match pool_map.get_mut(&PoolIdentifier {
                db: client.pool_name(),
                user: client.username(),
            }) {
                Some(pool_stats) => {
                    // Update client state counter based on client state
                    match client.state() {
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
            // Try to find the pool for this server
            match pool_map.get_mut(&PoolIdentifier {
                db: server.pool_name(),
                user: server.username(),
            }) {
                Some(pool_stats) => {
                    // Update server state counter based on server state
                    match server.state() {
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

    /// Returns pool statistics map (no aggregation needed since virtual pools were removed).
    ///
    /// # Arguments
    ///
    /// * `pool_map` - A map of pool statistics
    ///
    /// # Returns
    ///
    /// The same HashMap (identity function for backward compatibility)
    fn aggregate_pool_stats(
        pool_map: HashMap<PoolIdentifier, PoolStats>,
    ) -> HashMap<PoolIdentifier, PoolStats> {
        pool_map
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
