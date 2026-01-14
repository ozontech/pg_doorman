//! Tests for protocol message handling.

use super::protocol::{
    insert_close_complete_after_last_close_complete, insert_close_complete_before_ready_for_query,
    insert_parse_complete_before_bind_complete,
};
use bytes::BytesMut;

// Helper to create ParseComplete message
fn parse_complete_msg() -> Vec<u8> {
    vec![b'1', 0, 0, 0, 4]
}

// Helper to create BindComplete message
fn bind_complete_msg() -> Vec<u8> {
    vec![b'2', 0, 0, 0, 4]
}

// Helper to create CloseComplete message
fn close_complete_msg() -> Vec<u8> {
    vec![b'3', 0, 0, 0, 4]
}

// Helper to create ReadyForQuery message
fn ready_for_query_msg(status: u8) -> Vec<u8> {
    vec![b'Z', 0, 0, 0, 5, status]
}

// Helper to create CommandComplete message
fn command_complete_msg(tag: &str) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(b'C');
    msg.extend_from_slice(&((4 + tag.len() + 1) as i32).to_be_bytes());
    msg.extend_from_slice(tag.as_bytes());
    msg.push(0);
    msg
}

// Helper to create ParameterDescription message (response to Describe for statement)
// Format: 't' + length (4) + param_count (2) + param_oids (4 each)
fn parameter_description_msg(param_count: u16) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(b't');
    // length = 4 (self) + 2 (count) + 4 * param_count (oids)
    let len = 4 + 2 + 4 * param_count as i32;
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&param_count.to_be_bytes());
    // Add dummy OIDs (int4 = 23) for each parameter
    for _ in 0..param_count {
        msg.extend_from_slice(&23i32.to_be_bytes());
    }
    msg
}

// Helper to create RowDescription message (response to Describe for statement/portal with columns)
// Format: 'T' + length (4) + field_count (2) + fields...
fn row_description_msg(field_count: u16) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(b'T');
    // For simplicity, create minimal fields with name "c" + null + table_oid(4) + col_attr(2) + type_oid(4) + type_len(2) + type_mod(4) + format(2)
    // Each field: name(2 bytes "c\0") + 18 bytes = 20 bytes per field
    let field_size = 2 + 4 + 2 + 4 + 2 + 4 + 2; // 20 bytes per field
    let len = 4 + 2 + field_size * field_count as i32;
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&field_count.to_be_bytes());
    for _ in 0..field_count {
        msg.push(b'c'); // field name
        msg.push(0); // null terminator
        msg.extend_from_slice(&0i32.to_be_bytes()); // table OID
        msg.extend_from_slice(&0i16.to_be_bytes()); // column attribute number
        msg.extend_from_slice(&23i32.to_be_bytes()); // type OID (int4)
        msg.extend_from_slice(&4i16.to_be_bytes()); // type length
        msg.extend_from_slice(&(-1i32).to_be_bytes()); // type modifier
        msg.extend_from_slice(&0i16.to_be_bytes()); // format code (text)
    }
    msg
}

// Helper to create NoData message (response to Describe when no rows returned)
fn no_data_msg() -> Vec<u8> {
    vec![b'n', 0, 0, 0, 4]
}

#[test]
fn test_insert_parse_complete_count_zero() {
    let buffer = BytesMut::from(&bind_complete_msg()[..]);
    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer.clone(), 0);

    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_parse_complete_single_bind_without_parse() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&bind_complete_msg());

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [parse_complete_msg(), bind_complete_msg()].concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_single_bind_with_parse() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parse_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg());

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer.clone(), 1);

    // ParseComplete already present, no insertion needed
    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_parse_complete_multiple_binds_without_parse() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg());

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 2);

    assert_eq!(inserted, 2);
    let expected = [
        parse_complete_msg(),
        bind_complete_msg(),
        parse_complete_msg(),
        bind_complete_msg(),
        bind_complete_msg(),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_mixed_messages() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parse_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg()); // This needs ParseComplete
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        parse_complete_msg(),
        bind_complete_msg(),
        parse_complete_msg(),
        bind_complete_msg(),
        command_complete_msg("SELECT 1"),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_count_exceeds_available() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg());

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 10);

    // Only 2 BindComplete messages, so only 2 insertions
    assert_eq!(inserted, 2);
    let expected = [
        parse_complete_msg(),
        bind_complete_msg(),
        parse_complete_msg(),
        bind_complete_msg(),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_empty_buffer() {
    let buffer = BytesMut::new();
    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 1);

    assert_eq!(inserted, 0);
    assert_eq!(result.len(), 0);
}

#[test]
fn test_insert_parse_complete_no_bind_complete() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer.clone(), 1);

    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_close_complete_count_zero() {
    let buffer = BytesMut::from(&ready_for_query_msg(b'I')[..]);
    let result = insert_close_complete_before_ready_for_query(buffer.clone(), 0);

    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_close_complete_single_before_ready() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let result = insert_close_complete_before_ready_for_query(buffer, 1);

    let expected = [
        command_complete_msg("SELECT 1"),
        close_complete_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_multiple_before_ready() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let result = insert_close_complete_before_ready_for_query(buffer, 3);

    let expected = [
        command_complete_msg("SELECT 1"),
        close_complete_msg(),
        close_complete_msg(),
        close_complete_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_no_ready_for_query() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));

    let result = insert_close_complete_before_ready_for_query(buffer, 2);

    let expected = [
        command_complete_msg("SELECT 1"),
        close_complete_msg(),
        close_complete_msg(),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_empty_buffer() {
    let buffer = BytesMut::new();
    let result = insert_close_complete_before_ready_for_query(buffer, 1);

    let expected = close_complete_msg();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_only_ready_for_query() {
    let buffer = BytesMut::from(&ready_for_query_msg(b'T')[..]);
    let result = insert_close_complete_before_ready_for_query(buffer, 1);

    let expected = [close_complete_msg(), ready_for_query_msg(b'T')].concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_different_ready_statuses() {
    for status in [b'I', b'T', b'E'] {
        let buffer = BytesMut::from(&ready_for_query_msg(status)[..]);
        let result = insert_close_complete_before_ready_for_query(buffer, 1);

        let expected = [close_complete_msg(), ready_for_query_msg(status)].concat();
        assert_eq!(result.as_ref(), &expected[..]);
    }
}

#[test]
fn test_insert_parse_complete_complex_scenario() {
    // Simulate a batch of Parse/Bind operations
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parse_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg()); // Needs ParseComplete (1st)
    buffer.extend_from_slice(&parse_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&bind_complete_msg()); // Needs ParseComplete (2nd)
    buffer.extend_from_slice(&bind_complete_msg()); // Needs ParseComplete (3rd)

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 3);

    assert_eq!(inserted, 3);
    // Only first 3 BindComplete without preceding ParseComplete get it inserted
    let expected = [
        parse_complete_msg(),
        bind_complete_msg(),
        parse_complete_msg(), // Inserted before 2nd BindComplete
        bind_complete_msg(),
        parse_complete_msg(),
        bind_complete_msg(),
        parse_complete_msg(), // Inserted before 4th BindComplete
        bind_complete_msg(),
        parse_complete_msg(), // Inserted before 5th BindComplete
        bind_complete_msg(),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_with_multiple_messages() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&command_complete_msg("DELETE 5"));
    buffer.extend_from_slice(&close_complete_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let result = insert_close_complete_before_ready_for_query(buffer, 2);

    let expected = [
        bind_complete_msg(),
        command_complete_msg("DELETE 5"),
        close_complete_msg(),
        close_complete_msg(),
        close_complete_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

// Tests for insert_close_complete_after_last_close_complete

#[test]
fn test_insert_close_complete_after_last_count_zero() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&close_complete_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer.clone(), 0);

    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_close_complete_after_last_single() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&close_complete_msg());
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        close_complete_msg(),
        close_complete_msg(), // Inserted after last CloseComplete
        command_complete_msg("SELECT 1"),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_after_last_multiple() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&close_complete_msg());
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 3);

    assert_eq!(inserted, 3);
    let expected = [
        close_complete_msg(),
        close_complete_msg(), // Inserted 1
        close_complete_msg(), // Inserted 2
        close_complete_msg(), // Inserted 3
        command_complete_msg("SELECT 1"),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_after_last_no_close_complete() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 2);

    assert_eq!(inserted, 2);
    // No CloseComplete found, insert before ReadyForQuery
    let expected = [
        command_complete_msg("SELECT 1"),
        close_complete_msg(), // Inserted 1
        close_complete_msg(), // Inserted 2
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_after_last_empty_buffer() {
    let buffer = BytesMut::new();
    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 1);

    assert_eq!(inserted, 0);
    assert_eq!(result.len(), 0);
}

#[test]
fn test_insert_close_complete_after_last_no_ready_for_query() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer.clone(), 2);

    assert_eq!(inserted, 0);
    // No CloseComplete and no ReadyForQuery, return unchanged
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_close_complete_after_last_multiple_close_complete() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&close_complete_msg());
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&close_complete_msg()); // Last CloseComplete
    buffer.extend_from_slice(&command_complete_msg("SELECT 2"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 2);

    assert_eq!(inserted, 2);
    let expected = [
        close_complete_msg(),
        command_complete_msg("SELECT 1"),
        close_complete_msg(), // Last CloseComplete from server
        close_complete_msg(), // Inserted 1
        close_complete_msg(), // Inserted 2
        command_complete_msg("SELECT 2"),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_after_last_close_complete_at_end() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&close_complete_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        command_complete_msg("SELECT 1"),
        close_complete_msg(), // Last CloseComplete from server
        close_complete_msg(), // Inserted
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_after_last_complex_scenario() {
    // Simulate pipeline with interleaved operations
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&bind_complete_msg());
    buffer.extend_from_slice(&command_complete_msg("SELECT 2"));
    buffer.extend_from_slice(&close_complete_msg()); // CloseComplete for portal1
    buffer.extend_from_slice(&command_complete_msg("SELECT 3"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        bind_complete_msg(),
        command_complete_msg("SELECT 1"),
        bind_complete_msg(),
        command_complete_msg("SELECT 2"),
        close_complete_msg(), // CloseComplete for portal1 from server
        close_complete_msg(), // Inserted CloseComplete for stmt1
        command_complete_msg("SELECT 3"),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_close_complete_after_last_only_ready_for_query() {
    let buffer = BytesMut::from(&ready_for_query_msg(b'I')[..]);
    let (result, inserted) = insert_close_complete_after_last_close_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [close_complete_msg(), ready_for_query_msg(b'I')].concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

// Tests for ParseComplete insertion before ParameterDescription (Describe flow)

#[test]
fn test_insert_parse_complete_before_parameter_description() {
    // Simulate Describe response: ParameterDescription + RowDescription + ReadyForQuery
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1));
    buffer.extend_from_slice(&row_description_msg(1));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(1),
        row_description_msg(1),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_before_parameter_description_with_existing_parse() {
    // ParseComplete already present before ParameterDescription
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parse_complete_msg());
    buffer.extend_from_slice(&parameter_description_msg(2));
    buffer.extend_from_slice(&row_description_msg(1));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer.clone(), 1);

    // ParseComplete already present, no insertion needed
    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_parse_complete_before_no_data() {
    // Simulate Describe response for statement that returns no rows: NoData + ReadyForQuery
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&no_data_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        parse_complete_msg(),
        no_data_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_before_no_data_with_existing_parse() {
    // ParseComplete already present before NoData
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parse_complete_msg());
    buffer.extend_from_slice(&no_data_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer.clone(), 1);

    // ParseComplete already present, no insertion needed
    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_parse_complete_mixed_describe_and_bind() {
    // Simulate mixed scenario: Describe response followed by Bind response
    // This can happen in pipelined queries
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1)); // Needs ParseComplete
    buffer.extend_from_slice(&row_description_msg(1));
    buffer.extend_from_slice(&bind_complete_msg()); // Needs ParseComplete
    buffer.extend_from_slice(&command_complete_msg("SELECT 1"));
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 2);

    assert_eq!(inserted, 2);
    let expected = [
        parse_complete_msg(), // Inserted before ParameterDescription
        parameter_description_msg(1),
        row_description_msg(1),
        parse_complete_msg(), // Inserted before BindComplete
        bind_complete_msg(),
        command_complete_msg("SELECT 1"),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_parameter_description_in_second_chunk() {
    // Simulate scenario where ParameterDescription comes in a separate chunk
    // This is the bug scenario: first chunk has other messages, second has ParameterDescription
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(0)); // No params
    buffer.extend_from_slice(&row_description_msg(2));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(0),
        row_description_msg(2),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_multiple_describes() {
    // Multiple Describe responses in pipeline
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1)); // First Describe - needs ParseComplete
    buffer.extend_from_slice(&row_description_msg(1));
    buffer.extend_from_slice(&parameter_description_msg(2)); // Second Describe - needs ParseComplete
    buffer.extend_from_slice(&no_data_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_bind_complete(buffer, 2);

    assert_eq!(inserted, 2);
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(1),
        row_description_msg(1),
        parse_complete_msg(),
        parameter_description_msg(2),
        no_data_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}
