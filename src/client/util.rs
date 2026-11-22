use crate::app::errors::Error;
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

/// Combine deferred BEGIN and the next simple query into one Q message.
pub(crate) fn join_deferred_begin(
    mut begin: BytesMut,
    query: &BytesMut,
) -> Result<BytesMut, Error> {
    if !is_standalone_begin(&begin) || begin[BEGIN_MSG_LEN - 1] != 0 {
        return Err(Error::ProtocolSyncError(
            "invalid deferred BEGIN frame".to_string(),
        ));
    }
    if query.first() != Some(&b'Q') || query.len() < 6 {
        return Err(Error::ProtocolSyncError(
            "deferred BEGIN must be followed by a simple query".to_string(),
        ));
    }

    begin.truncate(BEGIN_MSG_LEN - 1);
    begin.extend_from_slice(b"\n;");
    begin.extend_from_slice(&query[5..]);
    let message_len = i32::try_from(begin.len() - 1).map_err(|_| Error::MaxMessageSize)?;
    begin[1..5].copy_from_slice(&message_len.to_be_bytes());
    Ok(begin)
}

/// Hide the real BEGIN completion after the client received a synthetic one.
/// A parse error has no BEGIN completion; forward that response unchanged.
pub(crate) fn strip_joined_begin_response(response: &mut BytesMut) -> Result<(), Error> {
    let mut offset = 0;
    while offset < response.len() {
        if response.len() - offset < 5 {
            return Err(Error::ProtocolSyncError(
                "short backend frame while hiding BEGIN".to_string(),
            ));
        }
        let message_len =
            u32::from_be_bytes(response[offset + 1..offset + 5].try_into().unwrap()) as usize;
        if message_len < 4 {
            return Err(Error::ProtocolSyncError(
                "invalid backend frame length while hiding BEGIN".to_string(),
            ));
        }
        let frame_end = offset
            .checked_add(1 + message_len)
            .filter(|end| *end <= response.len())
            .ok_or_else(|| {
                Error::ProtocolSyncError("truncated backend frame while hiding BEGIN".to_string())
            })?;

        match response[offset] {
            b'C' if &response[offset + 5..frame_end] == b"BEGIN\0" => {
                let removed = frame_end - offset;
                response.copy_within(frame_end.., offset);
                response.truncate(response.len() - removed);
                return Ok(());
            }
            // Joined SQL is parsed before BEGIN executes. On parse/analysis
            // failure the client gets the real error and RFQ(I); ROLLBACK
            // subsequently restores a usable idle session.
            b'E' => return Ok(()),
            b'Z' => {
                return Err(Error::ProtocolSyncError(
                    "missing BEGIN completion in joined response".to_string(),
                ));
            }
            _ => {}
        }
        offset = frame_end;
    }

    Err(Error::ProtocolSyncError(
        "BEGIN completion not found in joined response".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(sql: &[u8]) -> BytesMut {
        let mut message = BytesMut::from(&[b'Q', 0, 0, 0, 0][..]);
        message.extend_from_slice(sql);
        message.extend_from_slice(&[0]);
        let len = (message.len() - 1) as i32;
        message[1..5].copy_from_slice(&len.to_be_bytes());
        message
    }

    #[test]
    fn joins_begin_and_query_as_one_frame() {
        let joined = join_deferred_begin(query(b"BEGIN;"), &query(b"SELECT 1")).unwrap();
        assert_eq!(joined, query(b"BEGIN;\n;SELECT 1"));
        assert!(join_deferred_begin(query(b"BEGIN;"), &query(b"")).is_ok());
    }

    #[test]
    fn rejects_malformed_deferred_begin() {
        let mut begin = query(b"BEGIN;");
        begin[11] = 1;
        assert!(join_deferred_begin(begin, &query(b"SELECT 1")).is_err());
    }

    #[test]
    fn strips_only_begin_completion() {
        let mut response = BytesMut::from(
            &[
                b'C', 0, 0, 0, 10, b'B', b'E', b'G', b'I', b'N', 0, b'D', 0, 0, 0, 4, b'Z', 0, 0,
                0, 5, b'T',
            ][..],
        );
        strip_joined_begin_response(&mut response).unwrap();
        assert_eq!(&response[..], &[b'D', 0, 0, 0, 4, b'Z', 0, 0, 0, 5, b'T']);
    }

    #[test]
    fn forwards_parse_error_without_begin_completion() {
        let mut response = BytesMut::from(&[b'E', 0, 0, 0, 4, b'Z', 0, 0, 0, 5, b'I'][..]);
        let expected = response.clone();
        strip_joined_begin_response(&mut response).unwrap();
        assert_eq!(response, expected);
    }
}
