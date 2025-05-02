//! Errors.

/// Various errors.
#[derive(Debug, PartialEq, Clone)]
pub enum Error {
    SocketError(String),
    ClientSocketError(String, ClientIdentifier),
    ClientGeneralError(String, ClientIdentifier),
    ClientBadStartup,
    ProtocolSyncError(String),
    BadQuery(String),
    ServerError,
    ServerMessageParserError(String),
    ServerStartupError(String, ServerIdentifier),
    BadConnection,
    ServerAuthError(String, ServerIdentifier),
    ServerStartupReadParameters(String),
    BadConfig(String),
    AllServersDown,
    QueryWaitTimeout,
    ClientError(String),
    TlsError,
    StatementTimeout,
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
    MaxMessageSize,
    CurrentMemoryUsage,
    JWTPubKey(String),
    JWTPrivKey(String),
    JWTValidate(String),
    ProxyTimeout,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ClientIdentifier {
    pub addr: String,
    pub application_name: String,
    pub username: String,
    pub pool_name: String,
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
            &Error::ClientSocketError(error, client_identifier) => write!(
                f,
                "Error reading {} from client {}",
                error, client_identifier
            ),
            &Error::ClientGeneralError(error, client_identifier) => {
                write!(f, "{} {}", error, client_identifier)
            }
            &Error::ServerStartupError(error, server_identifier) => write!(
                f,
                "Error reading {} on server startup {}",
                error, server_identifier,
            ),
            &Error::ServerAuthError(error, server_identifier) => {
                write!(f, "{} for {}", error, server_identifier,)
            }

            // The rest can use Debug.
            err => write!(f, "{:?}", err),
        }
    }
}

impl From<std::ffi::NulError> for Error {
    fn from(err: std::ffi::NulError) -> Self {
        Error::QueryError(err.to_string())
    }
}
