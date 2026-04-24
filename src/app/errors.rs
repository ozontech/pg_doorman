//! Errors.

// Standard library imports

use crate::auth::hba::CheckResult;

/// Various errors.
#[derive(Debug, PartialEq, Clone)]
pub enum Error {
    SocketError(String),
    /// TCP or Unix socket connect() failed. Backend process unreachable.
    /// Distinct from SocketError, which covers read/write/protocol failures
    /// on an already established connection.
    ConnectError(String),
    ClientBadStartup,
    ProtocolSyncError(String),
    BadQuery(String),
    ServerError,
    ServerMessageParserError(String),
    ServerStartupError(String, ServerIdentifier),
    ServerAuthError(String, ServerIdentifier),
    /// PG startup FATAL with SQLSTATE class 57P (operator intervention):
    /// 57P01 admin_shutdown, 57P02 crash_shutdown, 57P03 cannot_connect_now.
    /// Backend accepted the connection but is not serving queries.
    ServerUnavailableError(String, ServerIdentifier),
    ServerStartupReadParameters(String),
    BadConfig(String),
    AllServersDown,
    QueryWaitTimeout,
    ClientError(String),
    TlsError,
    DNSCachedError(String),
    ShuttingDown,
    ParseBytesError(String),
    AuthError(String),
    UnsupportedStatement,
    QueryError(String),
    ScramClientError(String),
    ScramServerError(String),
    HbaForbiddenError(String),
    PreparedStatementError,
    FlushTimeout,
    MaxMessageSize,
    CurrentMemoryUsage,
    JWTPubKey(String),
    JWTPrivKey(String),
    JWTValidate(String),
    ProxyTimeout,
    ConvertError(String),
    /// PostgreSQL unreachable, connection lost. Transient — retry on next request.
    AuthQueryConnectionError(String),
    /// Bad query, wrong columns, >1 row, executor auth failed. Permanent until config fix.
    AuthQueryConfigError(String),
    /// SQL execution failed (permissions, PG overloaded). May be transient or permanent.
    AuthQueryQueryError(String),
    /// Executor pool is closed (shutting down).
    AuthQueryPoolClosed,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ClientIdentifier {
    pub addr: String,
    pub application_name: String,
    pub username: String,
    pub pool_name: String,
    pub is_talos: bool,
    pub hba_scram: CheckResult,
    pub hba_md5: CheckResult,
}

impl ClientIdentifier {
    pub fn new(
        application_name: &str,
        username: &str,
        pool_name: &str,
        addr: &str,
    ) -> ClientIdentifier {
        ClientIdentifier {
            addr: addr.into(),
            application_name: application_name.into(),
            username: username.into(),
            pool_name: pool_name.into(),
            is_talos: false,
            hba_scram: CheckResult::NotMatched,
            hba_md5: CheckResult::NotMatched,
        }
    }
}

impl std::fmt::Display for ClientIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{{ {}@{}/{}?application_name={} }}",
            self.username, self.addr, self.pool_name, self.application_name
        )
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct ServerIdentifier {
    pub username: String,
    pub database: String,
    pub pool_name: String,
}

impl ServerIdentifier {
    pub fn new(username: String, database: &str, pool_name: &str) -> ServerIdentifier {
        ServerIdentifier {
            username,
            database: database.into(),
            pool_name: pool_name.into(),
        }
    }
}

impl std::fmt::Display for ServerIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{{ username: {}, database: {} }}",
            self.username, self.database
        )
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self {
            Error::SocketError(msg) => write!(f, "Socket connection error: {msg}"),
            Error::ConnectError(msg) => write!(f, "Backend connect error: {msg}"),
            Error::ClientBadStartup => write!(f, "Client sent an invalid startup message"),
            Error::ProtocolSyncError(msg) => write!(f, "Protocol synchronization error: {msg}"),
            Error::BadQuery(msg) => write!(f, "Invalid query: {msg}"),
            Error::ServerError => write!(f, "Server encountered an error"),
            Error::ServerMessageParserError(msg) => {
                write!(f, "Failed to parse server message: {msg}")
            }
            Error::ServerStartupError(error, server_identifier) => write!(
                f,
                "Error reading {error} on server startup {server_identifier}"
            ),
            Error::ServerAuthError(error, server_identifier) => {
                write!(f, "{error} for {server_identifier}")
            }
            Error::ServerUnavailableError(error, server_identifier) => {
                write!(f, "Backend unavailable: {error} for {server_identifier}")
            }
            Error::ServerStartupReadParameters(msg) => {
                write!(f, "Failed to read server parameters: {msg}")
            }
            Error::BadConfig(msg) => write!(f, "Configuration error: {msg}"),
            Error::AllServersDown => write!(f, "All database servers are currently unavailable"),
            Error::QueryWaitTimeout => write!(f, "Query wait timed out"),
            Error::ClientError(msg) => write!(f, "Client error: {msg}"),
            Error::TlsError => write!(f, "TLS connection error"),
            Error::DNSCachedError(msg) => write!(f, "DNS resolution error: {msg}"),
            Error::ShuttingDown => write!(f, "Connection pooler is shutting down"),
            Error::ParseBytesError(msg) => write!(f, "Failed to parse bytes: {msg}"),
            Error::AuthError(msg) => write!(f, "Authentication failed: {msg}"),
            Error::UnsupportedStatement => write!(f, "Unsupported SQL statement"),
            Error::QueryError(msg) => write!(f, "Query execution error: {msg}"),
            Error::ScramClientError(msg) => write!(f, "SCRAM client error: {msg}"),
            Error::ScramServerError(msg) => write!(f, "SCRAM server error: {msg}"),
            Error::HbaForbiddenError(msg) => {
                write!(f, "Connection rejected by HBA configuration: {msg}")
            }
            Error::PreparedStatementError => write!(f, "Error with prepared statement"),
            Error::FlushTimeout => write!(f, "Timeout while flushing data to client"),
            Error::MaxMessageSize => write!(f, "Message exceeds maximum allowed size"),
            Error::CurrentMemoryUsage => write!(f, "Operation would exceed memory limits"),
            Error::JWTPubKey(msg) => write!(f, "JWT public key error: {msg}"),
            Error::JWTPrivKey(msg) => write!(f, "JWT private key error: {msg}"),
            Error::JWTValidate(msg) => write!(f, "JWT validation error: {msg}"),
            Error::ProxyTimeout => write!(f, "Proxy operation timed out"),
            Error::ConvertError(msg) => write!(f, "Data conversion error: {msg}"),
            Error::AuthQueryConnectionError(msg) => {
                write!(f, "Auth query connection error: {msg}")
            }
            Error::AuthQueryConfigError(msg) => {
                write!(f, "Auth query configuration error: {msg}")
            }
            Error::AuthQueryQueryError(msg) => {
                write!(f, "Auth query execution error: {msg}")
            }
            Error::AuthQueryPoolClosed => write!(f, "Auth query executor pool is closed"),
        }
    }
}

impl From<std::ffi::NulError> for Error {
    fn from(err: std::ffi::NulError) -> Self {
        Error::QueryError(err.to_string())
    }
}
