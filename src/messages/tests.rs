// Tests for the messages module
// This file contains tests for the various message handling functions

// Standard library imports
use std::io::{Error as IoError, ErrorKind};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake};

// External crate imports
use bytes::{BufMut, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

// Internal crate imports
use crate::errors::Error;
use crate::messages::{
    data_row, data_row_nullable, error_message, parse_startup, ready_for_query,
    set_messages_right_place, DataType, PgErrorMsg,
};
use crate::messages::protocol::row_description;

// Mock implementation for AsyncReadExt
struct MockReader {
    data: Vec<Vec<u8>>,
    current_index: usize,
}

impl AsyncRead for MockReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), IoError>> {
        if self.current_index >= self.data.len() {
            return Poll::Ready(Err(IoError::new(ErrorKind::UnexpectedEof, "No more data")));
        }

        let data = &self.data[self.current_index];
        let to_copy = std::cmp::min(buf.remaining(), data.len());
        buf.put_slice(&data[..to_copy]);
        self.current_index += 1;

        Poll::Ready(Ok(()))
    }
}

// Mock implementation for AsyncWriteExt
struct MockWriter {
    written: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl AsyncWrite for MockWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, IoError>> {
        self.written.lock().unwrap().push(buf.to_vec());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
        Poll::Ready(Ok(()))
    }
}

// Helper function to run async tests
struct MockWaker;
impl Wake for MockWaker {
    fn wake(self: Arc<Self>) {}
}

// Tests for parse_startup function
#[test]
fn test_parse_startup_success() {
    // Create a test BytesMut with parameters including "user"
    let mut bytes = BytesMut::new();

    // Add required "user" parameter
    bytes.put_slice(b"user\0testuser\0");
    // Add optional parameters
    bytes.put_slice(b"database\0testdb\0");
    bytes.put_slice(b"application_name\0testapp\0");
    bytes.put_u8(0); // Final null terminator

    // Call the function being tested
    let result = parse_startup(bytes);

    // Verify the result
    assert!(result.is_ok());
    let params = result.as_ref().unwrap();

    // Verify parameters
    assert_eq!(params.len(), 3);
    assert_eq!(params.get("user"), Some(&"testuser".to_string()));
    assert_eq!(params.get("database"), Some(&"testdb".to_string()));
    assert_eq!(params.get("application_name"), Some(&"testapp".to_string()));
}

#[test]
fn test_parse_startup_missing_user() {
    // Create a test BytesMut without the required "user" parameter
    let mut bytes = BytesMut::new();

    // Add some parameters but not "user"
    bytes.put_slice(b"database\0testdb\0");
    bytes.put_slice(b"application_name\0testapp\0");
    bytes.put_u8(0); // Final null terminator

    // Call the function being tested
    let result = parse_startup(bytes);

    // Verify the result is an error
    assert!(result.is_err());
    match result {
        Err(Error::ClientBadStartup) => {} // Expected error
        _ => panic!("Expected ClientBadStartup error"),
    }
}

// Tests for error_message function
#[test]
fn test_error_message_detailed() {
    // Call the function being tested
    let result = error_message("Test error message", "28000");

    // Verify the result
    assert!(!result.is_empty());

    // Check message type is 'E' (Error)
    assert_eq!(result[0], b'E');

    // Check message contains our error message and code
    let message_bytes = &result[5..];
    let message_str = String::from_utf8_lossy(message_bytes);

    assert!(message_str.contains("Test error message"));
    assert!(message_str.contains("28000"));
    assert!(message_str.contains("FATAL"));
}

// Tests for row_description function with columns
#[test]
fn test_row_description_with_columns() {
    // Create test columns
    let columns = vec![
        ("id", DataType::Int4),
        ("name", DataType::Text),
        ("active", DataType::Bool),
    ];

    // Call the function being tested
    let result = row_description(&columns);

    // Verify the result
    assert!(!result.is_empty());

    // Check message type is 'T' (Row Description)
    assert_eq!(result[0], b'T');

    // Check the number of columns (should be 3)
    let column_count_bytes = &result[5..7];
    let column_count = i16::from_be_bytes([column_count_bytes[0], column_count_bytes[1]]);
    assert_eq!(column_count, 3);
}

// Tests for data_row function
#[test]
fn test_data_row_with_values() {
    // Create test row data
    let row = vec!["1".to_string(), "Test Name".to_string(), "true".to_string()];

    // Call the function being tested
    let result = data_row(&row);

    // Verify the result
    assert!(!result.is_empty());

    // Check message type is 'D' (Data Row)
    assert_eq!(result[0], b'D');

    // Check the number of columns (should be 3)
    let column_count_bytes = &result[5..7];
    let column_count = i16::from_be_bytes([column_count_bytes[0], column_count_bytes[1]]);
    assert_eq!(column_count, 3);
}

// Tests for data_row_nullable function
#[test]
fn test_data_row_nullable_with_nulls() {
    // Create test row data with some null values
    let row = vec![Some("1".to_string()), None, Some("true".to_string())];

    // Call the function being tested
    let result = data_row_nullable(&row);

    // Verify the result
    assert!(!result.is_empty());

    // Check message type is 'D' (Data Row)
    assert_eq!(result[0], b'D');

    // Check the number of columns (should be 3)
    let column_count_bytes = &result[5..7];
    let column_count = i16::from_be_bytes([column_count_bytes[0], column_count_bytes[1]]);
    assert_eq!(column_count, 3);

    // The second value should be null (-1 length)
    // First find the position after the first value
    let mut pos = 7; // Start after column count
    let first_len_bytes = &result[pos..pos + 4];
    let first_len = i32::from_be_bytes([
        first_len_bytes[0],
        first_len_bytes[1],
        first_len_bytes[2],
        first_len_bytes[3],
    ]);
    pos += 4 + first_len as usize;

    // Now check the second value's length (should be -1 for NULL)
    let second_len_bytes = &result[pos..pos + 4];
    let second_len = i32::from_be_bytes([
        second_len_bytes[0],
        second_len_bytes[1],
        second_len_bytes[2],
        second_len_bytes[3],
    ]);
    assert_eq!(second_len, -1);
}

// Tests for ready_for_query function
#[test]
fn test_ready_for_query_states() {
    // Test with in_transaction = false
    let result_idle = ready_for_query(false);

    // Verify the result
    assert_eq!(result_idle.len(), 6);

    // Check message type is 'Z' (Ready For Query)
    assert_eq!(result_idle[0], b'Z');

    // Check transaction status is 'I' (Idle)
    assert_eq!(result_idle[5], b'I');

    // Test with in_transaction = true
    let result_transaction = ready_for_query(true);

    // Verify the result
    assert_eq!(result_transaction.len(), 6);

    // Check message type is 'Z' (Ready For Query)
    assert_eq!(result_transaction[0], b'Z');

    // Check transaction status is 'T' (In Transaction)
    assert_eq!(result_transaction[5], b'T');
}

// Tests for set_messages_right_place function
#[test]
fn test_set_messages_right_place_simple() {
    // Create a simple message
    let mut message = BytesMut::new();
    message.put_u8(b'T'); // Row Description
    message.put_i32(4); // Length

    // Call the function being tested
    let result = set_messages_right_place(message.to_vec());

    // Verify the result
    assert!(result.is_ok());
    let bytes = result.unwrap();

    // Check the message is preserved
    assert_eq!(bytes.len(), 5);
    assert_eq!(bytes[0], b'T');

    // Test with multiple messages
    let mut messages = BytesMut::new();
    messages.put_u8(b'T'); // Row Description
    messages.put_i32(4); // Length
    messages.put_u8(b'D'); // Data Row
    messages.put_i32(4); // Length

    // Call the function being tested
    let result = set_messages_right_place(messages.to_vec());

    // Verify the result
    assert!(result.is_ok());
    let bytes = result.unwrap();

    // Check both messages are preserved
    assert_eq!(bytes.len(), 10);
    assert_eq!(bytes[0], b'T');
    assert_eq!(bytes[5], b'D');
}

// Helper function for PgErrorMsg tests
fn field(kind: char, content: &str) -> Vec<u8> {
    format!("{kind}{content}\0").as_bytes().to_vec()
}

// Tests for PgErrorMsg parsing
#[test]
fn test_pg_error_msg_parsing() {
    let mut complete_msg = vec![];
    let severity = "FATAL";
    complete_msg.extend(field('S', severity));
    complete_msg.extend(field('V', severity));

    let error_code = "29P02";
    complete_msg.extend(field('C', error_code));
    let message = "password authentication failed for user \"wrong_user\"";
    complete_msg.extend(field('M', message));
    let detail_msg = "super detailed message";
    complete_msg.extend(field('D', detail_msg));
    let hint_msg = "hint detail here";
    complete_msg.extend(field('H', hint_msg));
    complete_msg.extend(field('P', "123"));
    complete_msg.extend(field('p', "234"));
    let internal_query = "SELECT * from foo;";
    complete_msg.extend(field('q', internal_query));
    let where_msg = "where goes here";
    complete_msg.extend(field('W', where_msg));
    let schema_msg = "schema_name";
    complete_msg.extend(field('s', schema_msg));
    let table_msg = "table_name";
    complete_msg.extend(field('t', table_msg));
    let column_msg = "column_name";
    complete_msg.extend(field('c', column_msg));
    let data_type_msg = "type_name";
    complete_msg.extend(field('d', data_type_msg));
    let constraint_msg = "constraint_name";
    complete_msg.extend(field('n', constraint_msg));
    let file_msg = "pg_doorman.c";
    complete_msg.extend(field('F', file_msg));
    complete_msg.extend(field('L', "335"));
    let routine_msg = "my_failing_routine";
    complete_msg.extend(field('R', routine_msg));

    let err_fields = PgErrorMsg::parse(&complete_msg).unwrap();

    assert_eq!(
        PgErrorMsg {
            severity_localized: severity.to_string(),
            severity: severity.to_string(),
            code: error_code.to_string(),
            message: message.to_string(),
            detail: Some(detail_msg.to_string()),
            hint: Some(hint_msg.to_string()),
            position: Some(123),
            internal_position: Some(234),
            internal_query: Some(internal_query.to_string()),
            where_context: Some(where_msg.to_string()),
            schema_name: Some(schema_msg.to_string()),
            table_name: Some(table_msg.to_string()),
            column_name: Some(column_msg.to_string()),
            data_type_name: Some(data_type_msg.to_string()),
            constraint_name: Some(constraint_msg.to_string()),
            file_name: Some(file_msg.to_string()),
            line: Some(335),
            routine: Some(routine_msg.to_string()),
        },
        err_fields
    );

    // Test with only mandatory fields
    let mut only_mandatory_msg = vec![];
    only_mandatory_msg.extend(field('S', severity));
    only_mandatory_msg.extend(field('V', severity));
    only_mandatory_msg.extend(field('C', error_code));
    only_mandatory_msg.extend(field('M', message));
    only_mandatory_msg.extend(field('D', detail_msg));

    let err_fields = PgErrorMsg::parse(&only_mandatory_msg).unwrap();

    assert_eq!(
        PgErrorMsg {
            severity_localized: severity.to_string(),
            severity: severity.to_string(),
            code: error_code.to_string(),
            message: message.to_string(),
            detail: Some(detail_msg.to_string()),
            hint: None,
            position: None,
            internal_position: None,
            internal_query: None,
            where_context: None,
            schema_name: None,
            table_name: None,
            column_name: None,
            data_type_name: None,
            constraint_name: None,
            file_name: None,
            line: None,
            routine: None,
        },
        err_fields
    );
}
