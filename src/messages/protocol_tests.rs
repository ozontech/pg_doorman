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

// ============================================================================
// Tests for insert_parse_complete_before_parameter_description
// ============================================================================

use super::protocol::insert_parse_complete_before_parameter_description;

// Helper to create ParameterDescription message ('t')
// Format: 't' + len(4) + num_params(2) + param_oids(4 * num_params)
fn parameter_description_msg(num_params: u16) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(b't');
    let len = 4 + 2 + (num_params as usize * 4);
    msg.extend_from_slice(&(len as i32).to_be_bytes());
    msg.extend_from_slice(&num_params.to_be_bytes());
    for _ in 0..num_params {
        msg.extend_from_slice(&23i32.to_be_bytes()); // INT4 OID
    }
    msg
}

// Helper to create NoData message ('n')
fn no_data_msg() -> Vec<u8> {
    vec![b'n', 0, 0, 0, 4]
}

// Helper to create RowDescription message ('T')
fn row_description_msg() -> Vec<u8> {
    // Minimal RowDescription with 0 columns
    vec![b'T', 0, 0, 0, 6, 0, 0]
}

#[test]
fn test_insert_parse_complete_before_param_desc_count_zero() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer.clone(), 0);

    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_parse_complete_before_param_desc_single() {
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(2));
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer, 1);

    assert_eq!(inserted, 1);
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(2),
        row_description_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_before_no_data_single() {
    // NoData ('n') alone means no ParameterDescription - nothing to insert before
    // This happens when Describe is for a portal, not a statement
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&no_data_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer.clone(), 1);

    // No 't' messages, so nothing inserted
    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_parse_complete_before_param_desc_multiple() {
    // Simulate multiple Describe responses: [t, T, t, T, Z]
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1));
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&parameter_description_msg(2));
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer, 2);

    assert_eq!(inserted, 2);
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(1),
        row_description_msg(),
        parse_complete_msg(),
        parameter_description_msg(2),
        row_description_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_before_param_desc_mixed_t_and_n() {
    // Simulate mixed responses: [t, T, t, n, Z] (one with RowDescription, one with NoData)
    // This is the actual batch PrepareAsync scenario: two statements, both have parameters
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1));
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&parameter_description_msg(2));
    buffer.extend_from_slice(&no_data_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer, 2);

    // Only insert before 't' (ParameterDescription), not before 'n' (NoData)
    assert_eq!(inserted, 2);
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(1),
        row_description_msg(),
        parse_complete_msg(),
        parameter_description_msg(2),
        no_data_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_before_param_desc_count_exceeds_available() {
    // Request 5 insertions but only 1 't' message available (we only insert before 't', not 'n')
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1));
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&no_data_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer, 5);

    assert_eq!(inserted, 1); // Only 1 't' available
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(1),
        row_description_msg(),
        no_data_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}

#[test]
fn test_insert_parse_complete_before_param_desc_empty_buffer() {
    let buffer = BytesMut::new();
    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer, 1);

    assert_eq!(inserted, 0);
    assert_eq!(result.len(), 0);
}

#[test]
fn test_insert_parse_complete_before_param_desc_no_t_or_n() {
    // Buffer with only RowDescription and ReadyForQuery (no 't' or 'n')
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer.clone(), 1);

    assert_eq!(inserted, 0);
    assert_eq!(result.as_ref(), buffer.as_ref());
}

#[test]
fn test_insert_parse_complete_before_param_desc_complex_scenario() {
    // Simulate real Describe flow for 3 cached prepared statements:
    // Server responds with [t, T, t, T, t, n, Z] for 3 Describe commands
    let mut buffer = BytesMut::new();
    buffer.extend_from_slice(&parameter_description_msg(1)); // Describe 1: has params
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&parameter_description_msg(0)); // Describe 2: no params
    buffer.extend_from_slice(&row_description_msg());
    buffer.extend_from_slice(&parameter_description_msg(2)); // Describe 3: has params, no results
    buffer.extend_from_slice(&no_data_msg());
    buffer.extend_from_slice(&ready_for_query_msg(b'I'));

    let (result, inserted) = insert_parse_complete_before_parameter_description(buffer, 3);

    assert_eq!(inserted, 3);
    let expected = [
        parse_complete_msg(),
        parameter_description_msg(1),
        row_description_msg(),
        parse_complete_msg(),
        parameter_description_msg(0),
        row_description_msg(),
        parse_complete_msg(),
        parameter_description_msg(2),
        no_data_msg(),
        ready_for_query_msg(b'I'),
    ]
    .concat();
    assert_eq!(result.as_ref(), &expected[..]);
}
