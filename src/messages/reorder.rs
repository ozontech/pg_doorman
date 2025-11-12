use crate::errors::Error;
use crate::messages::{close_complete, parse_complete};
use bytes::{BufMut, BytesMut};

// PostgreSQL protocol message type constants
const PARSE_COMPLETE: u8 = b'1';
const BIND_COMPLETE: u8 = b'2';
const CLOSE_COMPLETE: u8 = b'3';
const PARAMETER_DESCRIPTION: u8 = b't';
const READY_FOR_QUERY: u8 = b'Z';
const MESSAGE_LENGTH_SIZE: usize = 4;
const MIN_MESSAGE_LENGTH: i32 = 4;
const MESSAGE_HEADER_SIZE: usize = 5; // 1 byte type + 4 bytes length

// Pre-allocated error messages
static ERROR_CANT_READ: &str = "Can't read i32 from server message";
static ERROR_INVALID_LENGTH: &str = "Invalid message length";
static ERROR_MESSAGE_TOO_LONG: &str = "Message len more than server message size";

/// Read message length from buffer at given position
#[inline]
fn read_message_length(buffer: &[u8], position: usize) -> Result<usize, Error> {
    // Bounds check
    let end_pos = position
        .checked_add(MESSAGE_LENGTH_SIZE)
        .ok_or_else(|| Error::ServerMessageParserError(ERROR_CANT_READ.to_string()))?;

    if end_pos > buffer.len() {
        return Err(Error::ServerMessageParserError(ERROR_CANT_READ.to_string()));
    }

    // Direct array indexing instead of slicing
    let length_bytes = [
        buffer[position],
        buffer[position + 1],
        buffer[position + 2],
        buffer[position + 3],
    ];

    let length = i32::from_be_bytes(length_bytes);

    if length < MIN_MESSAGE_LENGTH {
        return Err(Error::ServerMessageParserError(
            ERROR_INVALID_LENGTH.to_string(),
        ));
    }

    Ok((length - MIN_MESSAGE_LENGTH) as usize)
}

/// Process and reorder messages in the buffer (single pass)
fn process_message_reordering_single_pass(buffer: &[u8]) -> Result<BytesMut, Error> {
    let buffer_len = buffer.len();
    let mut position = 0;

    // First quick scan to count special messages
    let mut parse_complete_count = 0;
    let mut close_complete_count = 0;
    let mut temp_pos = 0;

    while temp_pos < buffer_len {
        let message_type = buffer[temp_pos];

        if message_type == PARSE_COMPLETE {
            parse_complete_count += 1;
        } else if message_type == CLOSE_COMPLETE {
            close_complete_count += 1;
        }

        temp_pos += 1;

        if temp_pos + MESSAGE_LENGTH_SIZE > buffer_len {
            break;
        }

        let msg_len = i32::from_be_bytes([
            buffer[temp_pos],
            buffer[temp_pos + 1],
            buffer[temp_pos + 2],
            buffer[temp_pos + 3],
        ]);

        if msg_len < MIN_MESSAGE_LENGTH {
            return Err(Error::ServerMessageParserError(
                ERROR_INVALID_LENGTH.to_string(),
            ));
        }

        let payload_len = (msg_len - MIN_MESSAGE_LENGTH) as usize;
        temp_pos += MESSAGE_LENGTH_SIZE + payload_len;

        if temp_pos > buffer_len {
            return Err(Error::ServerMessageParserError(
                ERROR_MESSAGE_TOO_LONG.to_string(),
            ));
        }
    }

    // Early exit if no special messages
    if parse_complete_count == 0 && close_complete_count == 0 {
        return Ok(BytesMut::from(buffer));
    }

    // Allocate with extra space for potential insertions
    let extra_capacity = (parse_complete_count + close_complete_count) * MESSAGE_HEADER_SIZE;
    let mut result = BytesMut::with_capacity(buffer_len + extra_capacity);
    let mut prev_message_type = 0u8;

    while position < buffer_len {
        let current_message_type = buffer[position];

        // Handle special message types
        match current_message_type {
            PARSE_COMPLETE => {
                if parse_complete_count == 0 || prev_message_type == PARSE_COMPLETE {
                    // Skip redundant ParseComplete
                    if position + MESSAGE_HEADER_SIZE > buffer_len {
                        return Err(Error::ServerMessageParserError(
                            ERROR_MESSAGE_TOO_LONG.to_string(),
                        ));
                    }
                    position += MESSAGE_HEADER_SIZE;
                    prev_message_type = current_message_type;
                    continue;
                }
                parse_complete_count -= 1;
            }
            BIND_COMPLETE | PARAMETER_DESCRIPTION => {
                if prev_message_type != PARSE_COMPLETE
                    && prev_message_type != BIND_COMPLETE
                    && parse_complete_count > 0
                {
                    result.put(parse_complete());
                    parse_complete_count -= 1;
                }
            }
            CLOSE_COMPLETE => {
                if close_complete_count == 1 {
                    // Skip single CloseComplete
                    if position + MESSAGE_HEADER_SIZE > buffer_len {
                        return Err(Error::ServerMessageParserError(
                            ERROR_MESSAGE_TOO_LONG.to_string(),
                        ));
                    }
                    position += MESSAGE_HEADER_SIZE;
                    prev_message_type = current_message_type;
                    continue;
                }
            }
            READY_FOR_QUERY => {
                if close_complete_count == 1 {
                    result.put(close_complete());
                }
            }
            _ => {}
        }

        prev_message_type = current_message_type;
        position += 1;

        let message_length = read_message_length(buffer, position)?;
        position += MESSAGE_LENGTH_SIZE;

        if position + message_length > buffer_len {
            return Err(Error::ServerMessageParserError(
                ERROR_MESSAGE_TOO_LONG.to_string(),
            ));
        }

        // Copy entire message (type + length + data) in one operation
        let start = position - MESSAGE_HEADER_SIZE;
        let end = position + message_length;
        result.put(&buffer[start..end]);
        position = end;
    }

    Ok(result)
}

/// Reorder messages to ensure they are in the correct order.
#[inline]
pub fn set_messages_right_place(in_msg: Vec<u8>) -> Result<BytesMut, Error> {
    // Quick check: if buffer is too small to contain special messages, return as-is
    if in_msg.len() < MESSAGE_HEADER_SIZE {
        return Ok(BytesMut::from(&in_msg[..]));
    }

    process_message_reordering_single_pass(&in_msg)
}

/// Fast check if message reordering is needed
#[inline]
pub fn needs_message_reordering(buffer: &BytesMut) -> bool {
    // Quick size check
    if buffer.len() < MESSAGE_HEADER_SIZE {
        return false;
    }

    let bytes = buffer.as_ref();
    let mut pos = 0;
    let len = bytes.len();

    // Quick scan for special message types
    while pos + MESSAGE_HEADER_SIZE <= len {
        let msg_type = bytes[pos];

        // Found a message that needs reordering
        if msg_type == PARSE_COMPLETE || msg_type == CLOSE_COMPLETE {
            return true;
        }

        // Skip to next message
        pos += 1;

        // Read message length (4 bytes, big-endian)
        let msg_len =
            i32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);

        // Validate length
        if msg_len < 4 || msg_len as usize > len {
            break;
        }

        pos += 4 + (msg_len as usize - 4);
    }

    false
}

/// Reorder messages in-place to avoid extra allocations
#[inline]
pub fn reorder_messages_in_place(buffer: BytesMut) -> Result<BytesMut, Error> {
    // Quick check: small buffers likely don't need reordering
    if buffer.len() < 10 {
        return Ok(buffer);
    }

    // Convert to vec for reordering (unavoidable for now)
    // TODO: Future optimization - true in-place reordering
    set_messages_right_place(buffer.to_vec())
}
