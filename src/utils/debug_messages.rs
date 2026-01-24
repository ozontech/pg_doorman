use log::{debug, warn};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Maximum number of messages to buffer before flushing
const MAX_BUFFER_SIZE: usize = 100;

/// Maximum time to hold messages before flushing (100ms)
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum iterations when parsing messages to prevent infinite loops
const MAX_PARSE_ITERATIONS: usize = 10000;

/// Protocol state for tracking expected responses
#[derive(Debug, Clone, Default)]
struct ProtocolState {
    /// Number of pending Parse commands (expecting ParseComplete '1')
    pending_parse: u32,
    /// Number of pending Bind commands (expecting BindComplete '2')
    pending_bind: u32,
    /// Number of pending Describe commands (expecting 't'/'T'/'n')
    pending_describe: u32,
    /// Number of pending Execute commands (expecting DataRow/CommandComplete)
    pending_execute: u32,
    /// Number of pending Sync commands (expecting ReadyForQuery 'Z')
    pending_sync: u32,
    /// Number of pending Close commands (expecting CloseComplete '3')
    pending_close: u32,
    /// Whether we're in a simple query (Q) - expecting results then Z
    in_simple_query: bool,
    /// Whether we're receiving DataRows (after Execute or Query)
    receiving_data: bool,
}

impl ProtocolState {
    /// Reset state (e.g., after connection reset or error)
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Check if we have any pending operations
    fn has_pending(&self) -> bool {
        self.pending_parse > 0
            || self.pending_bind > 0
            || self.pending_describe > 0
            || self.pending_execute > 0
            || self.pending_sync > 0
            || self.pending_close > 0
            || self.in_simple_query
    }

    /// Process a client->server message and update expected responses
    fn process_client_message(&mut self, msg_type: char) {
        match msg_type {
            'P' => self.pending_parse += 1,
            'B' => self.pending_bind += 1,
            'D' => self.pending_describe += 1, // Describe from client
            'E' => {
                self.pending_execute += 1;
                self.receiving_data = true;
            }
            'C' => self.pending_close += 1, // Close from client
            'S' => self.pending_sync += 1,
            'H' => {} // Flush - no response expected
            'Q' => {
                self.in_simple_query = true;
                self.receiving_data = true;
            }
            'X' => self.reset(), // Terminate - reset state
            _ => {}
        }
    }

    /// Process a server->client message and check for protocol violations
    /// Returns a warning message if a violation is detected
    fn process_server_message(&mut self, msg_type: char) -> Option<String> {
        match msg_type {
            '1' => {
                // ParseComplete
                // Note: pg_doorman may insert extra ParseComplete for skipped (cached) Parse messages,
                // so receiving ParseComplete without pending Parse is valid and not a protocol violation.
                if self.pending_parse > 0 {
                    self.pending_parse -= 1;
                }
                None
            }
            '2' => {
                // BindComplete
                if self.pending_bind > 0 {
                    self.pending_bind -= 1;
                    None
                } else {
                    Some("BindComplete('2') received but no Bind was pending".to_string())
                }
            }
            '3' => {
                // CloseComplete
                if self.pending_close > 0 {
                    self.pending_close -= 1;
                    None
                } else {
                    Some("CloseComplete('3') received but no Close was pending".to_string())
                }
            }
            't' | 'T' | 'n' => {
                // ParameterDescription, RowDescription, NoData - responses to Describe
                // Also T can come after Execute/Query
                if self.pending_describe > 0 {
                    if msg_type == 'T' || msg_type == 'n' {
                        // RowDescription or NoData completes the Describe
                        self.pending_describe -= 1;
                    }
                    // 't' (ParameterDescription) is followed by T or n
                    None
                } else if self.receiving_data || self.in_simple_query {
                    // T can also come as part of query results
                    None
                } else {
                    Some(format!(
                        "{}('{}') received unexpectedly (no Describe/Execute pending)",
                        match msg_type {
                            't' => "ParameterDescription",
                            'T' => "RowDescription",
                            'n' => "NoData",
                            _ => "Unknown",
                        },
                        msg_type
                    ))
                }
            }
            'D' => {
                // DataRow - expected after Execute or Query
                if self.receiving_data || self.in_simple_query {
                    None
                } else {
                    Some(
                        "DataRow('D') received but not expecting data (no Execute/Query pending)"
                            .to_string(),
                    )
                }
            }
            'C' => {
                // CommandComplete - ends Execute or Query result set
                if self.pending_execute > 0 {
                    self.pending_execute -= 1;
                    if self.pending_execute == 0 && !self.in_simple_query {
                        self.receiving_data = false;
                    }
                    None
                } else if self.in_simple_query {
                    // Multiple commands in simple query
                    None
                } else {
                    Some(
                        "CommandComplete('C') received but no Execute/Query was pending"
                            .to_string(),
                    )
                }
            }
            's' => {
                // PortalSuspended - Execute with row limit
                if self.pending_execute > 0 || self.receiving_data {
                    None
                } else {
                    Some("PortalSuspended('s') received but no Execute was pending".to_string())
                }
            }
            'Z' => {
                // ReadyForQuery - ends transaction/sync
                if self.pending_sync > 0 {
                    self.pending_sync -= 1;
                }
                if self.in_simple_query {
                    self.in_simple_query = false;
                    self.receiving_data = false;
                }
                // Z resets the receiving state
                self.receiving_data = false;
                None
            }
            'E' => {
                // ErrorResponse - can come anytime, resets pending state until Sync
                // Don't reset completely, wait for Z
                self.receiving_data = false;
                None
            }
            'I' => {
                // EmptyQueryResponse
                if self.pending_execute > 0 {
                    self.pending_execute -= 1;
                    None
                } else if self.in_simple_query {
                    None
                } else {
                    Some("EmptyQueryResponse('I') received unexpectedly".to_string())
                }
            }
            // Other messages that don't affect protocol state tracking
            'S' | 'K' | 'N' | 'A' | 'G' | 'H' | 'W' | 'V' | 'R' | 'c' | 'd' | 'v' => None,
            _ => None,
        }
    }

    /// Get a summary of pending operations for debugging
    fn pending_summary(&self) -> String {
        let mut parts = Vec::new();
        if self.pending_parse > 0 {
            parts.push(format!("{}xParse", self.pending_parse));
        }
        if self.pending_bind > 0 {
            parts.push(format!("{}xBind", self.pending_bind));
        }
        if self.pending_describe > 0 {
            parts.push(format!("{}xDescribe", self.pending_describe));
        }
        if self.pending_execute > 0 {
            parts.push(format!("{}xExecute", self.pending_execute));
        }
        if self.pending_sync > 0 {
            parts.push(format!("{}xSync", self.pending_sync));
        }
        if self.pending_close > 0 {
            parts.push(format!("{}xClose", self.pending_close));
        }
        if self.in_simple_query {
            parts.push("SimpleQuery".to_string());
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(",")
        }
    }
}

/// Global protocol state tracker per server connection (server_pid -> state)
/// We track by server_pid to detect when a new client gets a "dirty" connection
/// that still has pending operations from a previous client.
static PROTOCOL_STATES: Lazy<Mutex<HashMap<i32, ProtocolState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

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

/// Extracts raw message types (just the type characters) from a PostgreSQL protocol buffer.
/// Used for protocol state tracking.
fn extract_raw_message_types(buffer: &[u8]) -> Vec<char> {
    let mut types = Vec::new();
    let mut pos = 0;
    let mut iterations = 0;

    while pos + 5 <= buffer.len() {
        iterations += 1;
        if iterations > MAX_PARSE_ITERATIONS {
            break;
        }

        let msg_type = buffer[pos] as char;

        if !msg_type.is_ascii_graphic() {
            break;
        }

        let len = i32::from_be_bytes([
            buffer[pos + 1],
            buffer[pos + 2],
            buffer[pos + 3],
            buffer[pos + 4],
        ]);

        if len < 4 {
            break;
        }

        let len = len as usize;

        if pos + 1 + len > buffer.len() {
            break;
        }

        types.push(msg_type);
        pos += 1 + len;
    }

    types
}

/// Log a client->server message with grouping and protocol analysis
#[inline(always)]
pub fn log_client_to_server(client_addr: &str, server_pid: i32, buffer: &[u8]) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    let message_types = extract_message_types(buffer);

    // Update protocol state for this server connection
    {
        let mut states = PROTOCOL_STATES.lock();
        let state = states.entry(server_pid).or_default();

        // Check if server has pending operations from a previous client
        // This can happen when a client disconnects without completing the protocol
        if state.has_pending() {
            let pending = state.pending_summary();
            warn!(
                "PROTOCOL WARNING {} -> Server [{}]: Server has pending operations from previous client: {} (new request: {})",
                client_addr, server_pid, pending, message_types
            );
        }

        // Extract raw message types and update state
        let raw_types = extract_raw_message_types(buffer);
        for msg_type in raw_types {
            state.process_client_message(msg_type);
        }
    }

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

/// Log a server->client message with grouping and protocol violation detection
pub fn log_server_to_client(client_addr: &str, server_pid: i32, buffer: &[u8]) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    let message_types = extract_message_types(buffer);

    // Check for protocol violations based on server connection state
    let violations: Vec<String>;
    let pending_before: String;
    {
        let mut states = PROTOCOL_STATES.lock();
        let state = states.entry(server_pid).or_default();

        pending_before = state.pending_summary();

        // Extract raw message types and check for violations
        let raw_types = extract_raw_message_types(buffer);
        violations = raw_types
            .iter()
            .filter_map(|&msg_type| state.process_server_message(msg_type))
            .collect();
    }

    // Log violations as warnings
    for violation in &violations {
        warn!(
            "PROTOCOL VIOLATION {} <-> Server [{}]: {} (pending: {})",
            client_addr, server_pid, violation, pending_before
        );
    }

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

/// Clean up protocol state for a disconnected client
/// Note: We don't remove the state because the server connection may be reused
/// by another client. The state will be naturally cleaned up when the server
/// connection is closed or when ReadyForQuery resets the state.
pub fn cleanup_protocol_state(client_addr: &str, server_pid: i32) {
    let states = PROTOCOL_STATES.lock();
    if let Some(state) = states.get(&server_pid) {
        if state.has_pending() {
            warn!(
                "Client {} disconnected with pending protocol state: {} (server [{}])",
                client_addr,
                state.pending_summary(),
                server_pid
            );
        }
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
