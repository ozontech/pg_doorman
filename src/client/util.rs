use bytes::BytesMut;
use once_cell::sync::Lazy;
use std::sync::{atomic::AtomicUsize, Arc};

/// Incrementally count prepared statements
/// to avoid random conflicts in places where the random number generator is weak.
pub static PREPARED_STATEMENT_COUNTER: Lazy<Arc<AtomicUsize>> =
    Lazy::new(|| Arc::new(AtomicUsize::new(0)));

// Ignore deallocate queries from pgx.
pub(crate) static QUERY_DEALLOCATE: &[u8] = "deallocate ".as_bytes();

/// Size of Q message containing "begin;" or "BEGIN;"
/// Format: [Q:1][length:4][query:6][null:1] = 12 bytes
const BEGIN_MSG_LEN: usize = 12;

/// Checks if the message is a standalone BEGIN query (simple query protocol).
/// Micro-optimization: first checks message size (12 bytes), then content.
/// 
/// Q message format:
/// - Byte 0: 'Q' (0x51)
/// - Bytes 1-4: length in big-endian (11 = 4 + 6 + 1)
/// - Bytes 5-10: "begin;" or "BEGIN;"
/// - Byte 11: null terminator (0x00)
#[inline]
pub(crate) fn is_standalone_begin(message: &BytesMut) -> bool {
    // Fast path: check size first
    if message.len() != BEGIN_MSG_LEN || message[0] != b'Q' {
        return false;
    }
    
    // Bytes 5-10 contain "begin;" (without null terminator)
    let query = &message[5..11];
    query.eq_ignore_ascii_case(b"begin;")
}
