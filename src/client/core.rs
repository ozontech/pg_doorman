use crate::errors::Error;
/// Handle clients by pretending to be a PostgreSQL server.
use ahash::AHashMap;
use bytes::BytesMut;
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

/// State related to prepared statements handling.
/// Groups all fields needed for prepared statement caching and batch processing.
pub struct PreparedStatementState {
    /// Whether prepared statements are enabled for this client
    pub enabled: bool,

    /// Whether this client has ever used async protocol (Flush command)
    /// Once set to true, prepared statements caching is disabled for this client
    pub async_client: bool,

    /// Maximum number of prepared statements in cache (0 = unlimited).
    /// Protection against malicious clients that don't call DEALLOCATE.
    pub max_cache_size: usize,

    /// Mapping of client named prepared statement to rewritten parse messages
    pub cache: AHashMap<PreparedStatementKey, (Arc<Parse>, u64)>,

    /// Hash of the last anonymous prepared statement (for Bind to find the corresponding Parse)
    pub last_anonymous_hash: Option<u64>,

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
}

impl PreparedStatementState {
    /// Create a new PreparedStatementState with the given enabled flag and max cache size.
    /// max_cache_size = 0 means unlimited (no protection against malicious clients).
    pub fn new(enabled: bool, max_cache_size: usize) -> Self {
        Self {
            enabled,
            async_client: false,
            max_cache_size,
            cache: AHashMap::new(),
            last_anonymous_hash: None,
            skipped_parses: Vec::new(),
            batch_operations: Vec::new(),
            parses_sent_in_batch: 0,
            processed_response_counts: ResponseCounts::default(),
            pending_close_complete: 0,
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

    /// Prepared statements state (caching, batch operations, etc.)
    pub(crate) prepared: PreparedStatementState,

    pub(crate) max_memory_usage: u64,

    pub(crate) client_last_messages_in_tx: PooledBuffer,

    pub(crate) pooler_check_query_request_vec: Vec<u8>,

    /// Pending BEGIN message for deferred connection optimization.
    /// When client sends standalone "begin;", we synthesize response
    /// and defer actual BEGIN until next query arrives.
    pub(crate) client_pending_begin: Option<BytesMut>,
}

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
            .remove(&(self.process_id, self.secret_key));
    }
}

impl<S, T> Drop for Client<S, T> {
    fn drop(&mut self) {
        self.client_server_map
            .remove(&(self.process_id, self.secret_key));

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
