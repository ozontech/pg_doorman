//! Errors.

use std::{ffi::NulError, io, net::SocketAddr};

use md5::digest::{InvalidLength as InvalidMd5Length, MacError};
use openssl::error::ErrorStack;

use crate::{auth::AuthMethod, stats::socket::SocketInfoError};

/// Various errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("socket error ocurred: {0}")]
    SocketError(String),
    #[error(transparent)]
    Socket(#[from] SocketError),
    #[error("error reading {0} from {1}")]
    ClientSocketError(String, ClientIdentifier),
    #[error(transparent)]
    ClientGeneral(#[from] ClientGeneralError),
    #[error(transparent)]
    ClientBadStartup(#[from] ClientBadStartupError),
    #[error(transparent)]
    ProtocolSync(#[from] ProtocolSyncError),
    #[error(transparent)]
    Server(#[from] ServerError),
    #[error(transparent)]
    ServerMessageParse(#[from] ServerMessageParseError),
    #[error("Error reading {0} on server startup {1}")]
    ServerStartupError(String, ServerIdentifier),
    #[error(transparent)]
    ServerAuth(#[from] ServerAuthError),
    #[error("TODO")]
    BadConfig(String),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error("TODO")]
    StatementTimeout,
    #[error("shutting down")]
    ShuttingDown,
    #[error(transparent)]
    ParseBytes(#[from] ParseBytesError),
    #[error("TODO")]
    AuthError(String),
    #[error(transparent)]
    QueryError(#[from] NulError),
    #[error("TODO")]
    ScramClientError(String),
    #[error("TODO")]
    ScramServerError(String),
    // the error is boxed since it is huge
    #[error(transparent)]
    HbaForbidden(#[from] Box<HbaForbiddenError>),
    #[error("prepated statement not found")]
    NoPreparedStatement,
    #[error("max message size")]
    MaxMessageSize,
    #[error("memory limit reached")]
    MemoryLimitReached,
    #[error(transparent)]
    JwtPubKey(#[from] JwtPubKeyError),
    #[error(transparent)]
    JwtValidate(#[from] JwtValidateError),
    #[error("proxy timeout")]
    ProxyTimeout,
}

#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("failed to flush socket")]
    Flush(#[source] io::Error),
    #[error("failed to write to socket")]
    Write(#[source] io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolSyncError {
    #[error("unexpected startup code {0}")]
    UnexpectedStartupCode(i32),
    #[error("SCRAM")]
    Scram,
    #[error("bad Postges client ({})", if *tls { "TLS" } else { "plain" })]
    BadClient { tls: bool },
    #[error("invalid code, expected {expected} but got {actual}")]
    InvalidCode { expected: u8, actual: u8 },
    #[error("unprocessed message code {0} from server backend while startup")]
    UnprocessedCode(u8),
    #[error("server {server} unknown transaction state {transaction_state}")]
    UnknownTransactionState {
        // TODO: something smarter
        server: String,
        transaction_state: u8,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientBadStartupError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("no parameters were specified")]
    NoParams,
    #[error("numbers of parameter keys and values don't match")]
    UnevenParams,
    #[error("user parameter is not specified")]
    UserUnspecified,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientGeneralError {
    #[error("invalid pool name {pool_name:?} for {id}")]
    InvalidPoolName {
        id: ClientIdentifier,
        pool_name: String,
    },
    #[error("invalid password for {id}")]
    InvalidPassword { id: ClientIdentifier },
}

#[derive(Debug, thiserror::Error)]
pub enum ServerAuthError {
    #[error("invalid authentication code {code} for {id}")]
    InvalidAuthCode { id: ServerIdentifier, code: i32 },
    #[error("unsupported authentication method {method} for {id}")]
    UnsupportedMethod {
        id: ServerIdentifier,
        method: AuthMethod,
    },
    #[error(transparent)]
    JwtPrivKey(#[from] JwtPrivKeyError),
    #[error("authentication method {method} failed")]
    Io {
        method: AuthMethod,
        #[source]
        error: io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum JwtPrivKeyError {
    #[error(transparent)]
    OpenSsl(#[from] ErrorStack),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Jwt(#[from] jwt::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum JwtValidateError {
    #[error("no expiration")]
    NoExpiration,
    #[error("expiration")]
    Expiration,
    #[error("not before")]
    NotBefore,
    #[error(transparent)]
    Jwt(#[from] jwt::Error),
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct TlsError(#[from] native_tls::Error);

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error(transparent)]
    InvalidMd5Length(#[from] InvalidMd5Length),
    #[error(transparent)]
    Mac(#[from] MacError),
    #[error("unsupported SCRAM version {0:?}")]
    UnsupportedScramVersion(String),
    #[error(transparent)]
    SocketInfo(#[from] SocketInfoError),
    #[error("error message is empty")]
    EmptyErrorMessage,
    #[error("internal server error")]
    Internal,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerMessageParseError {
    #[error("failed to read i32 value from server message")]
    InvalidI32,
    #[error("message `len` is less than 4")]
    LenSmallerThan4(usize),
    #[error("cursor {cursor} exceeds message length {message}")]
    CursorOverflow { cursor: usize, message: usize },
    #[error(
        "message length {message} at cursor {cursor} exceeds received message length {received}"
    )]
    LenOverlow {
        received: usize,
        cursor: usize,
        message: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("missing user parameter on client startup")]
    NoUserParam,
    #[error("Invalid pool name {{ username: {username}, pool_name: {pool_name}, application_name: {application_name}, virtual pool id: {virtual_pool_id} }}")]
    InvalidPoolName {
        username: String,
        pool_name: String,
        application_name: String,
        virtual_pool_id: u16,
    },
    #[error("prepared statement {0:?} does not exist")]
    PreparedStatementNotFound(String),
    #[error("failed to store prepated statemtn {0:?}")]
    PreparesStatementStore(String),
}

#[derive(Debug, thiserror::Error)]
#[error("hba forbidden client {client} from address: {address}")]
pub struct HbaForbiddenError {
    pub client: ClientIdentifier,
    pub address: SocketAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum JwtPubKeyError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    OpenSsl(#[from] ErrorStack),
    #[error("key is not loaded")]
    KeyNotLoaded,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseBytesError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("string is not nul-terminated")]
    NoNul,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
        let Self {
            addr,
            application_name,
            username,
            pool_name,
        } = self;
        write!(
            f,
            "{{ {username}@{addr}/{pool_name}?application_name={application_name} }}",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
        let Self { username, database } = self;
        write!(f, "{{ username: {username}, database: {database} }}")
    }
}

impl From<HbaForbiddenError> for Error {
    fn from(value: HbaForbiddenError) -> Self {
        Self::from(Box::new(value))
    }
}
