use thiserror::Error;
/// The error type returned by methods in this crate.
#[derive(Error, Debug)]
pub enum Error<E> {
    /// Manager Errors
    #[error("{0}")]
    Inner(E),
    /// Timeout
    #[error("Time out in the connection pool (query_wait_timeout)")]
    Timeout,
    /// BadConn
    #[error("Bad connection (postgresql_login_timeout)")]
    BadConn,
    /// The pool has been closed or dropped
    #[error("The pool is closed")]
    PoolClosed,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Error<E> {
        Error::Inner(e)
    }
}
