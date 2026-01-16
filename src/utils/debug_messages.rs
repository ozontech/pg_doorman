use log::debug;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum number of messages to buffer before flushing
const MAX_BUFFER_SIZE: usize = 100;

/// Maximum time to hold messages before flushing (100ms)
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum iterations when parsing messages to prevent infinite loops
const MAX_PARSE_ITERATIONS: usize = 10000;

/// Buffered debug message for grouping
#[derive(Clone, Debug)]
struct BufferedMessage {
    direction: String,
    client_addr: String,
    server_pid: i32,
    message_types: String,
}

impl PartialEq for BufferedMessage {
    fn eq(&self, other: &Self) -> bool {
        self.direction == other.direction
            && self.client_addr == other.client_addr
            && self.server_pid == other.server_pid
            && self.message_types == other.message_types
    }
}

/// Group of identical messages with count
struct MessageGroup {
    message: BufferedMessage,
    count: usize,
}

/// Debug message buffer for grouping repeated messages
pub struct DebugMessageBuffer {
    groups: VecDeque<MessageGroup>,
    last_flush: Instant,
    total_count: usize,
}

impl Default for DebugMessageBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugMessageBuffer {
    pub fn new() -> Self {
        Self {
            groups: VecDeque::new(),
            last_flush: Instant::now(),
            total_count: 0,
        }
    }

    /// Add a message to the buffer, grouping with the last message if identical
    fn add(&mut self, message: BufferedMessage) {
        if let Some(last_group) = self.groups.back_mut() {
            if last_group.message == message {
                last_group.count += 1;
                self.total_count += 1;
                return;
            }
        }

        self.groups.push_back(MessageGroup { message, count: 1 });
        self.total_count += 1;
    }

    /// Check if buffer should be flushed
    fn should_flush(&self) -> bool {
        self.total_count >= MAX_BUFFER_SIZE || self.last_flush.elapsed() >= FLUSH_INTERVAL
    }

    /// Flush all buffered messages to log
    fn flush(&mut self) {
        for group in self.groups.drain(..) {
            let msg = &group.message;
            if group.count > 1 {
                debug!(
                    "{}x {} {} <-> Server [{}]: {}",
                    group.count, msg.direction, msg.client_addr, msg.server_pid, msg.message_types
                );
            } else {
                debug!(
                    "{} {} <-> Server [{}]: {}",
                    msg.direction, msg.client_addr, msg.server_pid, msg.message_types
                );
            }
        }
        self.total_count = 0;
        self.last_flush = Instant::now();
    }
}

/// Global debug message buffer (lazy initialized)
static DEBUG_BUFFER: Lazy<Mutex<DebugMessageBuffer>> =
    Lazy::new(|| Mutex::new(DebugMessageBuffer::new()));

/// Log a client->server message with grouping
pub fn log_client_to_server(client_addr: &str, server_pid: i32, buffer: &[u8]) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    let message_types = extract_message_types(buffer);
    let msg = BufferedMessage {
        direction: "->".to_string(),
        client_addr: client_addr.to_string(),
        server_pid,
        message_types,
    };

    let mut guard = DEBUG_BUFFER.lock();
    guard.add(msg);
    if guard.should_flush() {
        guard.flush();
    }
}

/// Log a server->client message with grouping
pub fn log_server_to_client(client_addr: &str, server_pid: i32, buffer: &[u8]) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    let message_types = extract_message_types(buffer);
    let msg = BufferedMessage {
        direction: "<-".to_string(),
        client_addr: client_addr.to_string(),
        server_pid,
        message_types,
    };

    let mut guard = DEBUG_BUFFER.lock();
    guard.add(msg);
    if guard.should_flush() {
        guard.flush();
    }
}

/// Force flush the debug buffer (call periodically or on shutdown)
pub fn flush_debug_buffer() {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    let mut guard = DEBUG_BUFFER.lock();
    if guard.total_count > 0 {
        guard.flush();
    }
}

/// Extracts message types from a PostgreSQL protocol buffer.
/// For Parse/Bind/Describe/Close distinguishes named and anonymous.
/// Returns a string like "[P(stmt1),B,D,E,S]" or "[P(*),B(*),E,S]" for anonymous.
///
/// Groups consecutive identical message types, e.g., "D,D,D,D" becomes "4xD".
pub fn extract_message_types(buffer: &[u8]) -> String {
    let mut messages: Vec<(char, Option<String>)> = Vec::new();
    let mut pos = 0;
    let mut iterations = 0;

    while pos + 5 <= buffer.len() {
        // Safety: prevent infinite loops on malformed data
        iterations += 1;
        if iterations > MAX_PARSE_ITERATIONS {
            break;
        }

        let msg_type = buffer[pos] as char;

        // Validate message type is a valid PostgreSQL protocol message type
        // Frontend messages: B, C, c, d, D, E, F, f, H, P, p, Q, S, X
        // Backend messages: 1, 2, 3, A, C, c, d, D, E, G, H, I, K, n, N, R, s, S, t, T, V, W, Z
        // We accept any printable ASCII character to be flexible
        if !msg_type.is_ascii_graphic() {
            // Invalid message type, stop parsing
            break;
        }

        // Read message length (big-endian i32)
        let len = i32::from_be_bytes([
            buffer[pos + 1],
            buffer[pos + 2],
            buffer[pos + 3],
            buffer[pos + 4],
        ]);

        // Validate length
        if len < 4 {
            // Length must be at least 4 (includes the length field itself)
            break;
        }

        let len = len as usize;

        // Check if we have enough data for this message
        if pos + 1 + len > buffer.len() {
            // Incomplete message, stop parsing
            break;
        }

        let detail = match msg_type {
            'P' => {
                // Parse: after len comes name (null-terminated)
                if pos + 5 < buffer.len() {
                    let name_start = pos + 5;
                    if buffer[name_start] == 0 {
                        Some("*".to_string()) // anonymous
                    } else {
                        let name = read_cstring(&buffer[name_start..]);
                        if name.len() > 20 {
                            Some(format!("{}...", &name[..20]))
                        } else {
                            Some(name)
                        }
                    }
                } else {
                    None
                }
            }
            'B' => {
                // Bind: portal (null-term) + prepared_statement (null-term)
                if pos + 5 < buffer.len() {
                    let portal_start = pos + 5;
                    let portal_end = find_null(&buffer[portal_start..]);
                    let stmt_start = portal_start + portal_end + 1;
                    if stmt_start < buffer.len() {
                        if buffer[stmt_start] == 0 {
                            Some("*".to_string()) // anonymous
                        } else {
                            let name = read_cstring(&buffer[stmt_start..]);
                            if name.len() > 20 {
                                Some(format!("{}...", &name[..20]))
                            } else {
                                Some(name)
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            // Note: 'D' and 'C' are ambiguous:
            // - Client 'D' = Describe, 'C' = Close (have name fields)
            // - Server 'D' = DataRow, 'C' = CommandComplete (different format)
            // We cannot distinguish them without context, so we don't extract names for these.
            // This is acceptable for debug logging purposes.
            _ => None,
        };

        messages.push((msg_type, detail));
        pos += 1 + len;
    }

    format_grouped_messages(&messages)
}

/// Format messages with grouping for consecutive identical types
fn format_grouped_messages(messages: &[(char, Option<String>)]) -> String {
    if messages.is_empty() {
        return "[]".to_string();
    }

    let mut result = String::with_capacity(64);
    result.push('[');

    let mut i = 0;
    let mut first = true;

    while i < messages.len() {
        let (msg_type, detail) = &messages[i];

        // Count consecutive identical messages (same type and detail)
        let mut count = 1;
        while i + count < messages.len() {
            let (next_type, next_detail) = &messages[i + count];
            if next_type == msg_type && next_detail == detail {
                count += 1;
            } else {
                break;
            }
        }

        if !first {
            result.push(',');
        }
        first = false;

        if count > 1 {
            result.push_str(&format!("{}x", count));
        }

        result.push(*msg_type);

        if let Some(name) = detail {
            result.push('(');
            result.push_str(name);
            result.push(')');
        }

        i += count;
    }

    result.push(']');
    result
}

/// Reads a C-string (null-terminated) from buffer
fn read_cstring(buf: &[u8]) -> String {
    let end = find_null(buf);
    String::from_utf8_lossy(&buf[..end]).to_string()
}

/// Finds position of null byte
fn find_null(buf: &[u8]) -> usize {
    buf.iter().position(|&b| b == 0).unwrap_or(buf.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_simple_query() {
        // Simple Query 'Q' message: Q + len(4) + "SELECT 1\0"
        let query = b"SELECT 1\0";
        let len = (4 + query.len()) as i32;
        let mut buf = vec![b'Q'];
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(query);

        let result = extract_message_types(&buf);
        assert_eq!(result, "[Q]");
    }

    #[test]
    fn test_extract_anonymous_parse() {
        // Parse message with anonymous statement: P + len + \0 (empty name) + query + params
        let mut buf = vec![b'P'];
        let query = b"SELECT 1\0";
        let len = (4 + 1 + query.len() + 2) as i32; // len + empty name + query + num_params
        buf.extend_from_slice(&len.to_be_bytes());
        buf.push(0); // empty name (anonymous)
        buf.extend_from_slice(query);
        buf.extend_from_slice(&[0, 0]); // num_params = 0

        let result = extract_message_types(&buf);
        assert_eq!(result, "[P(*)]");
    }

    #[test]
    fn test_extract_named_parse() {
        // Parse message with named statement
        let mut buf = vec![b'P'];
        let name = b"my_stmt\0";
        let query = b"SELECT 1\0";
        let len = (4 + name.len() + query.len() + 2) as i32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(name);
        buf.extend_from_slice(query);
        buf.extend_from_slice(&[0, 0]); // num_params = 0

        let result = extract_message_types(&buf);
        assert_eq!(result, "[P(my_stmt)]");
    }

    #[test]
    fn test_extract_multiple_messages() {
        // Sync message: S + len(4)
        let mut buf = vec![];

        // Execute: E + len + portal + max_rows
        buf.push(b'E');
        let e_len = (4 + 1 + 4) as i32; // len + empty portal + max_rows
        buf.extend_from_slice(&e_len.to_be_bytes());
        buf.push(0); // empty portal
        buf.extend_from_slice(&0i32.to_be_bytes()); // max_rows = 0

        // Sync
        buf.push(b'S');
        buf.extend_from_slice(&4i32.to_be_bytes());

        let result = extract_message_types(&buf);
        assert_eq!(result, "[E,S]");
    }

    #[test]
    fn test_group_repeated_datarows() {
        // Multiple DataRow messages from server
        let mut buf = vec![];

        // Add 5 identical DataRow messages
        for _ in 0..5 {
            buf.push(b'D');
            let len = 4 + 2 + 4 + 1; // len + num_cols + col_len + data
            buf.extend_from_slice(&(len as i32).to_be_bytes());
            buf.extend_from_slice(&1i16.to_be_bytes()); // 1 column
            buf.extend_from_slice(&1i32.to_be_bytes()); // col length = 1
            buf.push(b'X'); // data
        }

        let result = extract_message_types(&buf);
        assert_eq!(result, "[5xD]");
    }

    #[test]
    fn test_group_mixed_messages() {
        // T, D, D, D, C, Z pattern
        let mut buf = vec![];

        // RowDescription 'T'
        buf.push(b'T');
        buf.extend_from_slice(&6i32.to_be_bytes());
        buf.extend_from_slice(&0i16.to_be_bytes()); // 0 columns

        // 3 DataRows
        for _ in 0..3 {
            buf.push(b'D');
            buf.extend_from_slice(&6i32.to_be_bytes());
            buf.extend_from_slice(&0i16.to_be_bytes()); // 0 columns
        }

        // CommandComplete 'C'
        buf.push(b'C');
        let tag = b"SELECT 3\0";
        buf.extend_from_slice(&((4 + tag.len()) as i32).to_be_bytes());
        buf.extend_from_slice(tag);

        // ReadyForQuery 'Z'
        buf.push(b'Z');
        buf.extend_from_slice(&5i32.to_be_bytes());
        buf.push(b'I'); // idle

        let result = extract_message_types(&buf);
        assert_eq!(result, "[T,3xD,C,Z]");
    }

    #[test]
    fn test_empty_buffer() {
        let result = extract_message_types(&[]);
        assert_eq!(result, "[]");
    }

    #[test]
    fn test_incomplete_message() {
        // Only 3 bytes, not enough for a complete message header
        let buf = vec![b'Q', 0, 0];
        let result = extract_message_types(&buf);
        assert_eq!(result, "[]");
    }

    #[test]
    fn test_invalid_length() {
        // Message with length < 4 (invalid)
        let mut buf = vec![b'Q'];
        buf.extend_from_slice(&2i32.to_be_bytes()); // invalid length
        buf.extend_from_slice(b"test");

        let result = extract_message_types(&buf);
        assert_eq!(result, "[]");
    }

    #[test]
    fn test_buffer_grouping() {
        let mut buffer = DebugMessageBuffer::new();

        let msg1 = BufferedMessage {
            direction: "->".to_string(),
            client_addr: "127.0.0.1:1234".to_string(),
            server_pid: 100,
            message_types: "[P(*),B(*),E,S]".to_string(),
        };

        // Add same message 3 times
        buffer.add(msg1.clone());
        buffer.add(msg1.clone());
        buffer.add(msg1.clone());

        assert_eq!(buffer.groups.len(), 1);
        assert_eq!(buffer.groups[0].count, 3);
        assert_eq!(buffer.total_count, 3);

        // Add different message
        let msg2 = BufferedMessage {
            direction: "<-".to_string(),
            client_addr: "127.0.0.1:1234".to_string(),
            server_pid: 100,
            message_types: "[1,2,T,3xD,C,Z]".to_string(),
        };
        buffer.add(msg2);

        assert_eq!(buffer.groups.len(), 2);
        assert_eq!(buffer.total_count, 4);
    }
}
