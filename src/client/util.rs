use bytes::BytesMut;
use once_cell::sync::Lazy;
use std::sync::{atomic::AtomicUsize, Arc};

/// Incrementally count prepared statements
/// to avoid random conflicts in places where the random number generator is weak.
pub static PREPARED_STATEMENT_COUNTER: Lazy<Arc<AtomicUsize>> =
    Lazy::new(|| Arc::new(AtomicUsize::new(0)));

pub static CLIENT_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));

// Ignore deallocate queries from pgx.
pub(crate) static QUERY_DEALLOCATE: &[u8] = "deallocate ".as_bytes();

const INITIAL_BUFFER_SIZE: usize = 8196;
const BUFFER_SHRINK_THRESHOLD: usize = 4 * INITIAL_BUFFER_SIZE; // 32KB

pub(crate) fn shrink_buffer_if_needed(buffer: &mut BytesMut) {
    if buffer.capacity() > BUFFER_SHRINK_THRESHOLD {
        let new_buffer = BytesMut::with_capacity(INITIAL_BUFFER_SIZE);
        *buffer = new_buffer;
    }
}
