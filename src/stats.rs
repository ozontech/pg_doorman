/// Statistics and reporting system for the PostgreSQL connection pooler.
///
/// This module provides a comprehensive statistics tracking system that monitors
/// various aspects of the connection pooler's operation, including:
///
/// - Client connections and their activities
/// - Server connections and their performance
/// - Connection pool usage and efficiency
/// - Query and transaction metrics
/// - Network throughput
///
/// The statistics are collected in real-time and periodically processed to calculate
/// averages and other derived metrics. These statistics can be queried through
/// administrative commands like SHOW CLIENTS and SHOW SERVERS.
use arc_swap::ArcSwap;
use log::{info, warn};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

// Sub-modules for different statistics components
// -----------------------------------------------------------------------------
/// Statistics for connections grouped by address
pub mod address;
/// Statistics for client connections
pub mod client;
/// Connection counters (internal)
mod connections;
/// Percentile calculation utilities (internal)
mod percenitle;
/// Statistics for connection pools
pub mod pool;
/// Utilities for printing statistics (internal)
pub mod print_all_stats;
/// Statistics for server connections
pub mod server;
/// Socket-related statistics (Linux only)
#[cfg(target_os = "linux")]
pub mod socket;

// Public exports for commonly used types and functions
// -----------------------------------------------------------------------------
use crate::stats::print_all_stats::print_all_stats;
pub use address::AddressStats;
pub use client::ClientStats;
pub use connections::{
    CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};
pub use server::ServerStats;
#[cfg(target_os = "linux")]
pub use socket::get_socket_states_count;

// Type definitions and global state
// -----------------------------------------------------------------------------
/// Type alias for the client statistics lookup table.
/// Maps client IDs to their corresponding statistics objects.
type ClientStatesLookup = HashMap<i32, Arc<ClientStats>>;

/// Type alias for the server statistics lookup table.
/// Maps server IDs to their corresponding statistics objects.
type ServerStatesLookup = HashMap<i32, Arc<ServerStats>>;

/// Global registry of client statistics.
///
/// This static variable maintains a thread-safe collection of all active client
/// connections and their associated statistics. It is used by the SHOW CLIENTS
/// administrative command to display information about connected clients.
static CLIENT_STATS: Lazy<Arc<RwLock<ClientStatesLookup>>> =
    Lazy::new(|| Arc::new(RwLock::new(ClientStatesLookup::default())));

/// Global registry of server statistics.
///
/// This static variable maintains a thread-safe collection of all active server
/// connections and their associated statistics. It is used by the SHOW SERVERS
/// administrative command to display information about server connections.
static SERVER_STATS: Lazy<Arc<RwLock<ServerStatesLookup>>> =
    Lazy::new(|| Arc::new(RwLock::new(ServerStatesLookup::default())));

/// Global statistics reporter instance.
///
/// This static variable provides a thread-safe reference to the statistics reporter.
/// The reporter is responsible for registering and unregistering clients and servers
/// with the statistics system.
pub static REPORTER: Lazy<ArcSwap<Reporter>> =
    Lazy::new(|| ArcSwap::from_pointee(Reporter::default()));

/// Statistics collection period in milliseconds.
///
/// This value determines how frequently statistics are collected and averages are
/// calculated. The current value is 15 seconds (15000 milliseconds).
static STAT_PERIOD: u64 = 15000;

/// Statistics reporter for registering and unregistering statistics sources.
///
/// The Reporter is responsible for managing the lifecycle of statistics objects
/// in the global registries. It provides methods for registering new clients and
/// servers when they connect, and for removing them when they disconnect.
///
/// An instance of this reporter is given to each possible source of statistics,
/// such as clients, servers, and connection pools.
#[derive(Clone, Debug, Default)]
pub struct Reporter {}

impl Reporter {
    /// Registers a client with the statistics system.
    ///
    /// This method adds a client's statistics object to the global registry, making it
    /// available for tracking and reporting. The client_id is used as a unique identifier
    /// to track and aggregate statistics from all sources related to that client.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Unique identifier for the client
    /// * `stats` - Arc-wrapped ClientStats instance to register
    ///
    /// # Note
    ///
    /// If a client with the same ID is already registered, a warning is logged and
    /// the registration is ignored to prevent overwriting existing statistics.
    fn client_register(&self, client_id: i32, stats: Arc<ClientStats>) {
        if CLIENT_STATS.read().get(&client_id).is_some() {
            warn!("Client {client_id:?} was double registered!");
            return;
        }

        CLIENT_STATS.write().insert(client_id, stats);
    }

    /// Unregisters a client from the statistics system.
    ///
    /// This method removes a client's statistics object from the global registry
    /// when the client disconnects from the pooler.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Unique identifier for the client to unregister
    fn client_disconnecting(&self, client_id: i32) {
        CLIENT_STATS.write().remove(&client_id);
    }

    /// Registers a server connection with the statistics system.
    ///
    /// This method adds a server's statistics object to the global registry, making it
    /// available for tracking and reporting. The server_id is used as a unique identifier
    /// to track and aggregate statistics from all sources related to that server.
    ///
    /// # Arguments
    ///
    /// * `server_id` - Unique identifier for the server
    /// * `stats` - Arc-wrapped ServerStats instance to register
    fn server_register(&self, server_id: i32, stats: Arc<ServerStats>) {
        SERVER_STATS.write().insert(server_id, stats);
    }

    /// Unregisters a server connection from the statistics system.
    ///
    /// This method removes a server's statistics object from the global registry
    /// when the server disconnects from the pooler.
    ///
    /// # Arguments
    ///
    /// * `server_id` - Unique identifier for the server to unregister
    fn server_disconnecting(&self, server_id: i32) {
        SERVER_STATS.write().remove(&server_id);
    }
}

/// Statistics collector for calculating and updating averages.
///
/// The Collector is responsible for periodically processing the raw statistics
/// data to calculate averages and other derived metrics. It runs as a background
/// task that wakes up at regular intervals (defined by STAT_PERIOD) to perform
/// these calculations.
///
/// There is only one collector instance in the system, which acts as a singleton
/// to ensure consistent statistics processing.
#[derive(Default)]
pub struct Collector {}

impl Collector {
    /// Starts the statistics collection process.
    ///
    /// This method spawns a background task that periodically:
    /// 1. Updates the average statistics for all server connections
    /// 2. Resets the current period counters for the next collection cycle
    /// 3. Prints all statistics for monitoring purposes
    ///
    /// The collection happens every STAT_PERIOD milliseconds (15 seconds by default).
    ///
    /// # Returns
    ///
    /// This method returns immediately after spawning the background task.
    pub async fn collect(&mut self) {
        info!("Events reporter started");

        tokio::task::spawn(async move {
            // Create a periodic interval for statistics collection
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_millis(STAT_PERIOD));

            loop {
                // Wait for the next interval
                interval.tick().await;

                // Process server statistics
                // Hold read lock for duration of update to retain all server stats
                {
                    let server_stats = SERVER_STATS.read();

                    // Update averages for each server that hasn't been updated yet
                    for stats in server_stats.values() {
                        if !stats.check_address_stat_average_is_updated_status() {
                            stats.address_stats().update_averages();
                            stats.address_stats().reset_current_counts();
                            stats.set_address_stat_average_is_updated_status(true);
                        }
                    }

                    // Reset the update status flags for the next cycle
                    for stats in server_stats.values() {
                        stats.set_address_stat_average_is_updated_status(false);
                    }
                }

                // Print all collected statistics
                print_all_stats();
            }
        });
    }
}

/// Gets a snapshot of all client statistics.
///
/// This function returns a copy of the current client statistics registry,
/// which can be used for reporting or analysis without affecting the
/// ongoing statistics collection.
///
/// # Returns
///
/// A HashMap mapping client IDs to their corresponding statistics objects
pub fn get_client_stats() -> ClientStatesLookup {
    CLIENT_STATS.read().clone()
}

/// Gets a snapshot of all server statistics.
///
/// This function returns a copy of the current server statistics registry,
/// which can be used for reporting or analysis without affecting the
/// ongoing statistics collection.
///
/// # Returns
///
/// A HashMap mapping server IDs to their corresponding statistics objects
pub fn get_server_stats() -> ServerStatesLookup {
    SERVER_STATS.read().clone()
}

/// Gets the global statistics reporter instance.
///
/// This function provides access to the statistics reporter, which is used
/// to register and unregister clients and servers with the statistics system.
///
/// # Returns
///
/// A clone of the global Reporter instance
pub fn get_reporter() -> Reporter {
    (*(*REPORTER.load())).clone()
}
