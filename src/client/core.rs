use crate::errors::Error;
/// Handle clients by pretending to be a PostgreSQL server.
use ahash::AHashMap;
use std::sync::Arc;
use tokio::io::BufReader;

use crate::client::buffer_pool::PooledBuffer;
use crate::messages::{error_response, Parse};
use crate::pool::{get_pool, ClientServerMap, ConnectionPool};
use crate::server::ServerParameters;
use crate::stats::{ClientStats, ServerStats};

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

    /// Counter for pending CloseComplete messages to send before ReadyForQuery
    pub(crate) pending_close_complete: u32,

    /// Counter for pending ParseComplete messages (for cached prepared statements)
    pub(crate) pending_parse_complete: u32,

    /// Counter for pending ParseComplete messages specifically for Describe flow
    /// (when Parse was skipped but Describe needs ParseComplete before ParameterDescription)
    pub(crate) pending_parse_complete_for_describe: u32,

    /// Address
    pub(crate) addr: std::net::SocketAddr,

    /// The client was started with the sole reason to cancel another running query.
    pub(crate) cancel_mode: bool,

    /// In transaction mode, the connection is released after each transaction.
    /// Session mode has slightly higher throughput per client, but lower capacity.
    pub(crate) transaction_mode: bool,

    /// For query cancellation, the client is given a random process ID and secret on startup.
    pub(crate) process_id: i32,
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

    /// Name of the server pool for this client (This comes from the database name in the connection string)
    pub(crate) pool_name: String,

    /// Postgres user for this client (This comes from the user in the connection string)
    pub(crate) username: String,

    /// Server startup and session parameters that we're going to track
    pub(crate) server_parameters: ServerParameters,

    /// Whether prepared statements are enabled for this client
    pub(crate) prepared_statements_enabled: bool,

    /// Whether this client has ever used async protocol (Flush command)
    /// Once set to true, prepared statements caching is disabled for this client
    pub(crate) async_client: bool,

    /// Mapping of client named prepared statement to rewritten parse messages
    pub(crate) prepared_statements: AHashMap<PreparedStatementKey, (Arc<Parse>, u64)>,

    /// Hash of the last anonymous prepared statement (for Bind to find the corresponding Parse)
    pub(crate) last_anonymous_prepared_hash: Option<u64>,

    pub(crate) max_memory_usage: u64,

    pub(crate) client_last_messages_in_tx: PooledBuffer,

    pub(crate) pooler_check_query_request_vec: Vec<u8>,
}

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    pub(crate) fn disconnect_stats(&self) {
        self.stats.disconnect();
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
    pub fn release(&self) {
        let mut guard = self.client_server_map.lock();
        guard.remove(&(self.process_id, self.secret_key));
    }
}

impl<S, T> Drop for Client<S, T> {
    fn drop(&mut self) {
        let mut guard = self.client_server_map.lock();
        guard.remove(&(self.process_id, self.secret_key));

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
