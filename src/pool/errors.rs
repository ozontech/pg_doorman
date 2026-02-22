use std::fmt;

use crate::errors::Error;

/// Possible errors returned by the recycle() method.
#[derive(Debug)]
pub enum RecycleError {
    /// Recycling failed for some reason.
    Message(String),

    /// Recycling failed for some reason (static message).
    StaticMessage(&'static str),

    /// Error caused by the backend.
    Backend(Error),
}

impl From<Error> for RecycleError {
    fn from(e: Error) -> Self {
        Self::Backend(e)
    }
}

impl fmt::Display for RecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(msg) => write!(
                f,
                "Error occurred while cleanup postgresql connection (postgresql_cleanup_error): {msg}"
            ),
            Self::StaticMessage(msg) => write!(
                f,
                "Error occurred while cleanup postgresql connection (postgresql_cleanup_error): {msg}"
            ),
            Self::Backend(e) => write!(
                f,
                "Error occurred while cleanup postgresql connection (postgresql_cleanup_error): {e}"
            ),
        }
    }
}

impl std::error::Error for RecycleError {}

/// Result type of the recycle() method.
pub type RecycleResult = Result<(), RecycleError>;

/// Possible steps causing the timeout in an error returned by Pool::get() method.
#[derive(Clone, Copy, Debug)]
pub enum TimeoutType {
    /// Timeout happened while waiting for a slot to become available.
    Wait,

    /// Timeout happened while creating a new object.
    Create,

    /// Timeout happened while recycling an object.
    Recycle,
}

/// Possible errors returned by Pool::get() method.
#[derive(Debug)]
pub enum PoolError {
    /// Timeout happened.
    Timeout(TimeoutType),

    /// Backend reported an error.
    Backend(Error),

    /// Pool has been closed.
    Closed,
}

impl From<Error> for PoolError {
    fn from(e: Error) -> Self {
        Self::Backend(e)
    }
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(tt) => match tt {
                TimeoutType::Wait => write!(
                    f,
                    "Timeout occurred while waiting free connection from pool (query_wait_timeout)"
                ),
                TimeoutType::Create => write!(
                    f,
                    "Timeout occurred while creating a new connection to postgresql (postgresql_login_timeout)"
                ),
                TimeoutType::Recycle => write!(
                    f,
                    "Timeout occurred while cleanup connection to postgresql (postgresql_cleanup_timeout)"
                ),
            },
            Self::Backend(e) => write!(
                f,
                "Error occurred while creating a new connection to postgresql (postgresql_login_error): {e}"
            ),
            Self::Closed => write!(f, "Pool has been closed"),
        }
    }
}

impl std::error::Error for PoolError {}
