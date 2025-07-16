//! Errors.

// Standard library imports

/// Various errors.
#[derive(Debug, PartialEq, Clone)]
pub enum Error {
    SocketError(String),
    ClientBadStartup,
    ProtocolSyncError(String),
    BadQuery(String),
    ServerError,
    ServerMessageParserError(String),
    ServerStartupError(String, ServerIdentifier),
    ServerAuthError(String, ServerIdentifier),
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
}

#[derive(Clone, PartialEq, Debug)]
pub struct ClientIdentifier {
    pub addr: String,
    pub application_name: String,
    pub username: String,
    pub pool_name: String,
    pub is_talos: bool,
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
}

impl ServerIdentifier {
    pub fn new(username: String, database: &str) -> ServerIdentifier {
        ServerIdentifier {
            username,
            database: database.into(),
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
        }
    }
}

impl From<std::ffi::NulError> for Error {
    fn from(err: std::ffi::NulError) -> Self {
        Error::QueryError(err.to_string())
    }
}
