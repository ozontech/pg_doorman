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
