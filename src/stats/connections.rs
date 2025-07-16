/// Connection counters for tracking different types of connections to the PostgreSQL pooler.
///
/// This module provides atomic counters that are incremented whenever a new connection
/// is established. These counters are used for monitoring and diagnostics purposes.
use once_cell::sync::Lazy;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

/// Total number of connections established since the pooler started.
///
/// This counter is incremented for every new connection, regardless of type.
pub static TOTAL_CONNECTION_COUNTER: Lazy<Arc<AtomicUsize>> =
    Lazy::new(|| Arc::new(AtomicUsize::new(0)));

/// Number of TLS/SSL encrypted connections established since the pooler started.
///
/// This counter is incremented for connections that use TLS/SSL encryption.
pub static TLS_CONNECTION_COUNTER: Lazy<Arc<AtomicUsize>> =
    Lazy::new(|| Arc::new(AtomicUsize::new(0)));

/// Number of plain (unencrypted) connections established since the pooler started.
///
/// This counter is incremented for connections that do not use encryption.
pub static PLAIN_CONNECTION_COUNTER: Lazy<Arc<AtomicUsize>> =
    Lazy::new(|| Arc::new(AtomicUsize::new(0)));

/// Number of cancel request connections established since the pooler started.
///
/// This counter is incremented for connections that are specifically for
/// canceling running queries (PostgreSQL cancel requests).
pub static CANCEL_CONNECTION_COUNTER: Lazy<Arc<AtomicUsize>> =
    Lazy::new(|| Arc::new(AtomicUsize::new(0)));
