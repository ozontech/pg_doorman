//! PostgreSQL protocol I/O operations for server connections.
//!
//! This module handles communication with PostgreSQL servers, including:
//! - Sending messages to the server with timeout support
//! - Receiving and parsing server responses
//! - Handling large messages and COPY protocol
//! - Managing server state based on protocol messages

use std::mem;
use std::time::{Duration, SystemTime};

use bytes::{Buf, BufMut, BytesMut};
use log::{error, info, warn};

/// Replace newlines and carriage returns to keep log lines single-line.
fn sanitize_for_log(s: &str) -> String {
    if s.contains(['\n', '\r']) {
        s.replace('\n', "\\n").replace('\r', "\\r")
    } else {
        s.to_string()
    }
}
use tokio::time::timeout;

use crate::config::get_config;
use crate::errors::Error;
use crate::errors::Error::MaxMessageSize;
use crate::messages::PgErrorMsg;
use crate::messages::MAX_MESSAGE_SIZE;
use crate::messages::{
    proxy_copy_data, proxy_copy_data_with_timeout, read_message_body_reuse, read_message_header,
    write_all_flush, BytesMutReader,
};

use super::parameters::ServerParameters;
use super::server_backend::Server;

// PostgreSQL CommandComplete message payloads for tracking session state changes.
//
// A checkin-time `RESET ALL` / `DEALLOCATE ALL` / `CLOSE ALL` is a heuristic
// upper bound: we arm the `needs_cleanup_*` flags when we see a statement that
// *might* have mutated the session, and we disarm them when we see a statement
// that has since restored it. Disarming matters because otherwise a client that
// performs its own reset batch (e.g. pgx on internal context deadline sends
// `SET SESSION AUTHORIZATION DEFAULT; RESET ALL; CLOSE ALL; UNLISTEN *;
// DISCARD PLANS; ...`) leaves pg_doorman thinking the connection is still dirty
// and triggers a second, redundant `RESET ALL` round-trip on checkin.

/// `SET` statement CommandComplete tag — arms the `needs_cleanup_set` flag.
/// Returned for any `SET foo = ...`, including `SET SESSION AUTHORIZATION ...`.
const COMMAND_COMPLETE_BY_SET: &[u8; 4] = b"SET\0";
/// `RESET` statement CommandComplete tag — disarms `needs_cleanup_set`.
/// PostgreSQL returns this tag both for `RESET ALL` and for `RESET foo.bar`;
/// the per-GUC form is still safe to treat as a reset because the only state
/// pg_doorman tracked is the `SET` flag — the next `SET` will re-arm it.
const COMMAND_COMPLETE_BY_RESET: &[u8; 6] = b"RESET\0";
/// `DECLARE CURSOR` CommandComplete tag — arms the `needs_cleanup_declare` flag.
const COMMAND_COMPLETE_BY_DECLARE: &[u8; 15] = b"DECLARE CURSOR\0";
/// `CLOSE ALL` CommandComplete tag — disarms `needs_cleanup_declare`.
/// Note the server emits `CLOSE CURSOR ALL`, not `CLOSE ALL`.
const COMMAND_COMPLETE_BY_CLOSE_CURSOR_ALL: &[u8; 17] = b"CLOSE CURSOR ALL\0";
/// `DEALLOCATE ALL` CommandComplete tag — clears prepared statement cache
/// and disarms `needs_cleanup_prepare`.
const COMMAND_COMPLETE_BY_DEALLOCATE_ALL: &[u8; 15] = b"DEALLOCATE ALL\0";
/// `DISCARD ALL` CommandComplete tag — equivalent to `RESET ALL; DEALLOCATE ALL;
/// CLOSE ALL; UNLISTEN *; ...`, so disarms every `needs_cleanup_*` flag.
const COMMAND_COMPLETE_BY_DISCARD_ALL: &[u8; 12] = b"DISCARD ALL\0";

/// Buffer flush threshold in bytes (8 KiB).
/// When the buffer reaches this size, it will be flushed to avoid excessive memory usage.
const BUFFER_FLUSH_THRESHOLD: usize = 8192;

// ============================================================================
// Public API functions
// ============================================================================

/// Sends messages to the server and flushes the write buffer with a timeout.
/// Returns an error if the operation doesn't complete within the specified duration.
pub(crate) async fn send_and_flush_timeout(
    server: &mut Server,
    messages: &BytesMut,
    duration: Duration,
) -> Result<(), Error> {
    match timeout(duration, send_and_flush(server, messages)).await {
        Ok(result) => result,
        Err(err) => {
            server.mark_bad("flush timeout");
            error!(
                "[{}@{}] flush timeout pid={}: {err}",
                server.address.username,
                server.address.pool_name,
                server.get_process_id(),
            );
            Err(Error::FlushTimeout)
        }
    }
}

/// Sends messages to the server and flushes the write buffer immediately.
/// Updates statistics and last activity timestamp on success.
/// Marks the connection as bad and logs an error on failure.
pub(crate) async fn send_and_flush(server: &mut Server, messages: &BytesMut) -> Result<(), Error> {
    server.stats.data_sent(messages.len());
    server.stats.wait_writing();

    match write_all_flush(&mut server.stream, messages).await {
        Ok(_) => {
            // Successfully sent to server
            server.stats.wait_idle();
            server.last_activity = SystemTime::now();
            Ok(())
        }
        Err(err) => {
            server.stats.wait_idle();
            error!(
                "[{}@{}] server connection terminated pid={}: {err}",
                server.address.username,
                server.address.pool_name,
                server.get_process_id(),
            );
            server.mark_bad("failed to flush data to server");
            Err(err)
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Handles large DataRow ('D') messages that exceed max_message_size.
/// Streams the message directly to the client without buffering.
async fn handle_large_data_row<C>(
    server: &mut Server,
    client_stream: &mut C,
    code_u8: u8,
    message_len: i32,
) -> Result<BytesMut, Error>
where
    C: tokio::io::AsyncWrite + std::marker::Unpin,
{
    // Send current buffer + header
    server.buffer.put_u8(code_u8);
    server.buffer.put_i32(message_len);
    let prev_bad = server.bad;
    server.bad = true;
    write_all_flush(client_stream, &server.buffer).await?;

    // Stream the large message directly
    match proxy_copy_data_with_timeout(
        get_config().general.proxy_copy_data_timeout.as_std(),
        &mut server.stream,
        client_stream,
        message_len as usize - mem::size_of::<i32>(),
    )
    .await
    {
        Ok(_) => (),
        Err(err) => {
            server.mark_bad(err.to_string().as_str());
            return Err(err);
        }
    }

    if !prev_bad {
        server.bad = false;
    }

    server
        .stats
        .data_received(server.buffer.len() + message_len as usize);
    server.last_activity = SystemTime::now();
    server.data_available = true;
    server.buffer.clear();
    server.stats.wait_idle();
    Ok(server.buffer.clone())
}

/// Handles large CopyData ('d') messages that exceed max_message_size.
/// Streams the message directly to the client without buffering.
async fn handle_large_copy_data<C>(
    server: &mut Server,
    client_stream: &mut C,
    code_u8: u8,
    message_len: i32,
) -> Result<BytesMut, Error>
where
    C: tokio::io::AsyncWrite + std::marker::Unpin,
{
    // Send current buffer + header
    server.buffer.put_u8(code_u8);
    server.buffer.put_i32(message_len);
    let prev_bad = server.bad;
    server.bad = true;
    write_all_flush(client_stream, &server.buffer).await?;

    // Stream the large message directly
    proxy_copy_data(
        &mut server.stream,
        client_stream,
        message_len as usize - mem::size_of::<i32>(),
    )
    .await?;

    server.bad = prev_bad;
    server
        .stats
        .data_received(server.buffer.len() + message_len as usize);
    server.last_activity = SystemTime::now();
    server.buffer.clear();
    server.stats.wait_idle();
    Ok(server.buffer.clone())
}

/// Handles ReadyForQuery ('Z') message - indicates server is ready for a new query.
/// Updates transaction state based on the transaction status indicator.
fn handle_ready_for_query(server: &mut Server, message: &mut BytesMut) -> Result<(), Error> {
    let transaction_state = message.get_u8() as char;

    match transaction_state {
        // 'T' - In transaction block
        'T' => {
            server.in_transaction = true;
        }

        // 'I' - Idle (not in transaction)
        'I' => {
            server.in_transaction = false;
        }

        // 'E' - In failed transaction block (requires ROLLBACK)
        'E' => {
            server.in_transaction = true;
            if let Ok(msg) = PgErrorMsg::parse(message) {
                let mut details =
                    format!(
                    "[{}@{}] transaction rolled back pid={}: severity={}, code={}, message=\"{}\"",
                    server.address.username, server.address.pool_name, server.get_process_id(),
                    msg.severity, msg.code, sanitize_for_log(&msg.message),
                );
                if let Some(ref hint) = msg.hint {
                    details.push_str(&format!(", hint=\"{}\"", sanitize_for_log(hint)));
                }
                error!("{details}");
            } else {
                error!(
                    "[{}@{}] transaction error pid={}: could not parse error details",
                    server.address.username,
                    server.address.pool_name,
                    server.get_process_id(),
                );
            }
        }

        // Unknown transaction state - protocol error
        _ => {
            let err = Error::ProtocolSyncError(format!(
                "Protocol synchronization error with server {} (database: {}, user: {}). Received unknown transaction state character: '{}' (ASCII: {}). This may indicate an incompatible PostgreSQL server version or a corrupted message.",
                server.address.host,
                server.address.database,
                server.address.username,
                transaction_state,
                transaction_state as u8
            ));
            error!("{err}");
            server.mark_bad(
                format!("Protocol sync error: unknown transaction state '{transaction_state}'")
                    .as_str(),
            );
            return Err(err);
        }
    };

    // No more data available from the server after ReadyForQuery
    server.data_available = false;
    Ok(())
}

/// Handles ErrorResponse ('E') message from the server.
/// Logs the error and updates server state accordingly.
fn handle_error_response(server: &mut Server, message: &mut BytesMut) {
    if let Ok(msg) = PgErrorMsg::parse(message) {
        let mut details = format!(
            "[{}@{}] server error pid={}: severity={}, code={}, message=\"{}\", in_transaction={}, in_copy={}",
            server.address.username, server.address.pool_name, server.get_process_id(),
            msg.severity, msg.code, sanitize_for_log(&msg.message),
            server.in_transaction, server.in_copy_mode,
        );
        if let Some(ref hint) = msg.hint {
            details.push_str(&format!(", hint=\"{}\"", sanitize_for_log(hint)));
        }
        if let Some(ref detail) = msg.detail {
            details.push_str(&format!(", detail=\"{}\"", sanitize_for_log(detail)));
        }
        error!("{details}");
    } else {
        error!(
            "[{}@{}] server error pid={}: could not parse error details",
            server.address.username,
            server.address.pool_name,
            server.get_process_id(),
        );
    }

    // Exit COPY mode on error
    if server.in_copy_mode {
        server.in_copy_mode = false;
    }

    // Reset prepared statements cache on error
    if server.prepared_statement_cache.is_some() {
        server.cleanup_state.needs_cleanup_prepare = true;
    }

    // Handle async mode errors
    if server.is_async() {
        server.data_available = false;
        server.cleanup_state.needs_cleanup();
        if !server.session_mode {
            server.mark_bad("PostgreSQL error in asynchronous operation mode");
        }
    }
}

/// Effect a single CommandComplete tag has on the server's cleanup tracking.
///
/// Extracted from [`handle_command_complete`] so the tag-matching logic can be
/// unit-tested without constructing a full `Server`. See the tests at the bottom
/// of this file for the exhaustive tag coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandCompleteEffect {
    /// Tag does not influence cleanup tracking (e.g. SELECT, INSERT).
    None,
    /// `SET ...` — session GUC potentially mutated; arm set-cleanup.
    ArmSet,
    /// `DECLARE CURSOR` — a server-side cursor may now be open; arm declare-cleanup.
    ArmDeclare,
    /// `RESET` / `RESET ALL` — session GUCs are back to the server defaults;
    /// disarm set-cleanup because the subsequent checkin RESET would be a no-op.
    DisarmSet,
    /// `CLOSE CURSOR ALL` — no server-side cursors remain; disarm declare-cleanup.
    DisarmDeclare,
    /// `DEALLOCATE ALL` — every prepared statement is gone server-side; disarm
    /// prepare-cleanup and drop the LRU so the next checkout starts from scratch.
    DisarmPrepare,
    /// `DISCARD ALL` — equivalent to `RESET ALL; DEALLOCATE ALL; CLOSE ALL;
    /// UNLISTEN *; ...` executed atomically; disarm every `needs_cleanup_*` flag
    /// and drop the LRU.
    DisarmAll,
}

/// Pure classifier for CommandComplete tags relevant to session cleanup tracking.
///
/// The tags are compared byte-for-byte; `PartialEq for [u8]` already short-circuits
/// on length, so non-matching messages (the common case on the hot path) cost a
/// single length comparison per arm.
fn classify_command_complete(tag: &[u8]) -> CommandCompleteEffect {
    if tag == COMMAND_COMPLETE_BY_SET {
        CommandCompleteEffect::ArmSet
    } else if tag == COMMAND_COMPLETE_BY_RESET {
        CommandCompleteEffect::DisarmSet
    } else if tag == COMMAND_COMPLETE_BY_DECLARE {
        CommandCompleteEffect::ArmDeclare
    } else if tag == COMMAND_COMPLETE_BY_CLOSE_CURSOR_ALL {
        CommandCompleteEffect::DisarmDeclare
    } else if tag == COMMAND_COMPLETE_BY_DEALLOCATE_ALL {
        CommandCompleteEffect::DisarmPrepare
    } else if tag == COMMAND_COMPLETE_BY_DISCARD_ALL {
        CommandCompleteEffect::DisarmAll
    } else {
        CommandCompleteEffect::None
    }
}

/// Drop the pg_doorman-side prepared statement LRU after the server confirms it
/// just executed an equivalent of `DEALLOCATE ALL` or `DISCARD ALL`.
fn drop_prepared_statement_cache_on_reset(server: &mut Server, reason: &'static str) {
    server.registering_prepared_statement.clear();
    let Some(cache_size) = server
        .prepared_statement_cache
        .as_ref()
        .map(|cache| cache.len())
    else {
        return;
    };
    warn!(
        "[{}@{}] clearing prepared statement cache pid={}: {reason} ({cache_size} entries)",
        server.address.username,
        server.address.pool_name,
        server.get_process_id(),
    );
    if let Some(cache) = server.prepared_statement_cache.as_mut() {
        cache.clear();
    }
}

/// Handles CommandComplete ('C') message - indicates successful completion of a command.
/// Tracks commands that may require cleanup (SET, DECLARE, ...) and disarms the
/// cleanup flags when the session has since been restored by a RESET / DISCARD /
/// DEALLOCATE / CLOSE ALL statement in the same or a later batch — so that the
/// next checkin does not issue a redundant `RESET ALL` round-trip on a connection
/// the client has already cleaned up.
fn handle_command_complete(server: &mut Server, message: &BytesMut) {
    // Exit COPY mode if we were in it
    if server.in_copy_mode {
        server.in_copy_mode = false;
    }

    match classify_command_complete(&message[..]) {
        CommandCompleteEffect::None => {}
        CommandCompleteEffect::ArmSet => {
            server.cleanup_state.needs_cleanup_set = true;
        }
        CommandCompleteEffect::ArmDeclare => {
            server.cleanup_state.needs_cleanup_declare = true;
        }
        CommandCompleteEffect::DisarmSet => {
            server.cleanup_state.needs_cleanup_set = false;
        }
        CommandCompleteEffect::DisarmDeclare => {
            server.cleanup_state.needs_cleanup_declare = false;
        }
        CommandCompleteEffect::DisarmPrepare => {
            server.cleanup_state.needs_cleanup_prepare = false;
            drop_prepared_statement_cache_on_reset(server, "DEALLOCATE ALL");
        }
        CommandCompleteEffect::DisarmAll => {
            server.cleanup_state.reset();
            drop_prepared_statement_cache_on_reset(server, "DISCARD ALL");
        }
    }
}

/// Handles ParameterStatus ('S') message - server runtime parameter change notification.
/// Updates both server and client parameter tracking.
fn handle_parameter_status(
    server: &mut Server,
    message: &mut BytesMut,
    client_server_parameters: &mut Option<&mut ServerParameters>,
) {
    let key = message.read_string().unwrap();
    let value = message.read_string().unwrap();

    // Update client parameters if tracking is enabled
    if let Some(client_server_parameters) = client_server_parameters.as_mut() {
        client_server_parameters.set_param(&key, &value, false);
        if server.log_client_parameter_status_changes {
            info!(
                "[{}@{}] parameter changed pid={}: {key}={value}",
                server.address.username,
                server.address.pool_name,
                server.get_process_id()
            )
        }
    }

    // Always update server parameters
    server.server_parameters.set_param(key, value, false);
}

/// Receive data from the server in response to a client request.
/// Must be called multiple times while `server.is_data_available()` is true.
pub(crate) async fn recv<C>(
    server: &mut Server,
    mut client_stream: C,
    mut client_server_parameters: Option<&mut ServerParameters>,
) -> Result<BytesMut, Error>
where
    C: tokio::io::AsyncWrite + std::marker::Unpin,
{
    // Handle deferred large message from previous recv() call.
    // When recv() encounters a large DataRow/CopyData but the buffer already has
    // accumulated messages, it returns the buffer first (for response reordering)
    // and saves the large message header here for the next call.
    if let Some((code_u8, message_len)) = server.pending_large_message.take() {
        match code_u8 as char {
            'D' => {
                return handle_large_data_row(server, &mut client_stream, code_u8, message_len)
                    .await
            }
            'd' => {
                return handle_large_copy_data(server, &mut client_stream, code_u8, message_len)
                    .await
            }
            _ => unreachable!("pending_large_message should only contain 'D' or 'd'"),
        }
    }

    loop {
        server.stats.wait_reading();

        // In async mode, check if all expected responses have been received
        if server.is_async() && server.expected_responses() == 0 {
            server.data_available = false;
            break;
        }

        let (code_u8, message_len) = read_message_header(&mut server.stream).await?;
        // Handle large DataRow messages that exceed max_message_size
        if server.max_message_size > 0
            && message_len > server.max_message_size
            && code_u8 as char == 'D'
        {
            // If buffer has accumulated messages (e.g. BindComplete, RowDescription),
            // return them first so execute_server_roundtrip can run
            // reorder_parse_complete_responses before we stream to client.
            if !server.buffer.is_empty() {
                server.pending_large_message = Some((code_u8, message_len));
                server.data_available = true;
                let result = server.buffer.clone();
                server.buffer.clear();
                server.stats.data_received(result.len());
                server.last_activity = SystemTime::now();
                return Ok(result);
            }
            return handle_large_data_row(server, &mut client_stream, code_u8, message_len).await;
        }

        // Handle large CopyData messages that exceed max_message_size
        if server.max_message_size > 0
            && message_len > server.max_message_size
            && code_u8 as char == 'd'
        {
            if !server.buffer.is_empty() {
                server.pending_large_message = Some((code_u8, message_len));
                server.data_available = true;
                let result = server.buffer.clone();
                server.buffer.clear();
                server.stats.data_received(result.len());
                server.last_activity = SystemTime::now();
                return Ok(result);
            }
            return handle_large_copy_data(server, &mut client_stream, code_u8, message_len).await;
        }

        if message_len > MAX_MESSAGE_SIZE {
            error!(
                "[{}@{}] message size limit exceeded pid={}: received={} bytes, max={} bytes",
                server.address.username,
                server.address.pool_name,
                server.get_process_id(),
                message_len,
                MAX_MESSAGE_SIZE,
            );
            server.mark_bad(
                format!(
                    "Message size limit exceeded: {message_len} bytes (max: {MAX_MESSAGE_SIZE} bytes)"
                )
                .as_str(),
            );
            return Err(MaxMessageSize);
        }

        // Read body into per-connection reusable buffer (header already consumed above).
        let mut message = match read_message_body_reuse(
            &mut server.stream,
            &mut server.read_buf,
            code_u8,
            message_len,
        )
        .await
        {
            Ok(message) => {
                server.stats.wait_idle();
                message
            }
            Err(err) => {
                error!(
                    "[{}@{}] server connection terminated pid={}: {err}",
                    server.address.username,
                    server.address.pool_name,
                    server.get_process_id(),
                );
                server.mark_bad(format!("Failed to read message data: {err}").as_str());
                return Err(err);
            }
        };

        // Buffer the message we'll forward to the client later.
        server.buffer.put(&message[..]);

        let code = message.get_u8() as char;
        let _len = message.get_i32();

        match code {
            // ReadyForQuery - server is ready for a new query
            'Z' => {
                handle_ready_for_query(server, &mut message)?;
                break;
            }

            // ErrorResponse - server encountered an error
            'E' => {
                handle_error_response(server, &mut message);
                // In async mode, error aborts remaining operations in pipeline
                if server.is_async() {
                    server.reset_expected_responses();
                }
            }

            // CommandComplete - command executed successfully
            'C' => {
                handle_command_complete(server, &message);
                // In async mode, this ends an Execute operation
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // ParameterStatus - server parameter changed
            'S' => {
                handle_parameter_status(server, &mut message, &mut client_server_parameters);
            }

            // DataRow
            'D' => {
                // More data is available after this message, this is not the end of the reply.
                server.data_available = true;

                // Don't flush yet, the more we buffer, the faster this goes...up to a limit.
                if server.buffer.len() >= BUFFER_FLUSH_THRESHOLD {
                    break;
                }
            }

            // CopyInResponse: copy is starting from client to server.
            'G' => {
                server.in_copy_mode = true;
                break;
            }

            // CopyOutResponse: copy is starting from the server to the client.
            'H' => {
                server.in_copy_mode = true;
                server.data_available = true;
                break;
            }

            // CopyData
            'd' => {
                // Don't flush yet, buffer until we reach limit
                if server.buffer.len() >= BUFFER_FLUSH_THRESHOLD {
                    break;
                }
            }

            // CopyDone
            // Buffer until ReadyForQuery shows up, so don't exit the loop yet.
            'c' => (),

            // ParseComplete
            // Response to Parse message in extended query protocol
            '1' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // BindComplete
            // Response to Bind message in extended query protocol
            '2' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // CloseComplete
            // Response to Close message in extended query protocol
            '3' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // ParameterDescription
            // Response to Describe message for a statement
            't' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // PortalSuspended
            // Indicates that Execute completed but portal still has rows
            's' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // NoData
            // Response to Describe when statement/portal produces no rows
            // https://www.postgresql.org/docs/current/protocol-flow.html
            'n' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // RowDescription
            // Response to Describe for a portal (or statement if it returns rows)
            'T' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // EmptyQueryResponse
            // Response to Execute with an empty query string
            'I' => {
                if server.is_async() {
                    server.decrement_expected();
                }
            }

            // Anything else, e.g. notices, etc.
            // Keep buffering until ReadyForQuery shows up.
            _ => (),
        };
    }

    let bytes = server.buffer.clone();

    // Keep track of how much data we got from the server for stats.
    server.stats.data_received(bytes.len());

    // Clear the buffer for next query.
    if server.buffer.len() > BUFFER_FLUSH_THRESHOLD {
        server.buffer = BytesMut::with_capacity(BUFFER_FLUSH_THRESHOLD);
    } else {
        // Clear the buffer for next query.
        server.buffer.clear();
    }

    // Successfully received data from server
    server.last_activity = SystemTime::now();

    // Pass the data back to the client.
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    //! Pure-function tests for CommandComplete tag classification.
    //!
    //! The tag strings were captured empirically against PostgreSQL 16 by
    //! connecting with `psql` and inspecting the CommandComplete payload —
    //! PostgreSQL does not expose the tag list as a public contract, so these
    //! tests pin the bytes pg_doorman relies on.
    //!
    //! Note the two non-obvious cases:
    //! * `RESET ALL` is reported as `RESET\0`, not `RESET ALL\0`.
    //! * `CLOSE ALL` is reported as `CLOSE CURSOR ALL\0`, not `CLOSE ALL\0`.

    use super::{classify_command_complete, CommandCompleteEffect};

    #[test]
    fn set_tag_arms_set_cleanup() {
        assert_eq!(
            classify_command_complete(b"SET\0"),
            CommandCompleteEffect::ArmSet,
        );
    }

    #[test]
    fn reset_tag_disarms_set_cleanup() {
        // PostgreSQL emits the same `RESET\0` tag for `RESET ALL` and
        // `RESET foo.bar`; both restore GUCs to their default so either one
        // legitimately disarms pg_doorman's heuristic flag.
        assert_eq!(
            classify_command_complete(b"RESET\0"),
            CommandCompleteEffect::DisarmSet,
        );
    }

    #[test]
    fn declare_cursor_tag_arms_declare_cleanup() {
        assert_eq!(
            classify_command_complete(b"DECLARE CURSOR\0"),
            CommandCompleteEffect::ArmDeclare,
        );
    }

    #[test]
    fn close_cursor_all_tag_disarms_declare_cleanup() {
        assert_eq!(
            classify_command_complete(b"CLOSE CURSOR ALL\0"),
            CommandCompleteEffect::DisarmDeclare,
        );
    }

    #[test]
    fn close_single_cursor_tag_is_inert() {
        // Closing one named cursor is not the same as `CLOSE ALL` — other
        // cursors may still be open, so this tag must NOT disarm declare-cleanup.
        assert_eq!(
            classify_command_complete(b"CLOSE CURSOR\0"),
            CommandCompleteEffect::None,
        );
    }

    #[test]
    fn deallocate_all_tag_disarms_prepare_cleanup() {
        assert_eq!(
            classify_command_complete(b"DEALLOCATE ALL\0"),
            CommandCompleteEffect::DisarmPrepare,
        );
    }

    #[test]
    fn discard_all_tag_disarms_every_cleanup_flag() {
        assert_eq!(
            classify_command_complete(b"DISCARD ALL\0"),
            CommandCompleteEffect::DisarmAll,
        );
    }

    #[test]
    fn partial_discard_tags_are_inert() {
        // DISCARD PLANS drops the plan cache, DISCARD TEMP drops temp tables,
        // DISCARD SEQUENCES resets sequence caches. None of them revert SET
        // state or drop prepared statements, so none should influence the
        // cleanup flags on their own.
        assert_eq!(
            classify_command_complete(b"DISCARD PLANS\0"),
            CommandCompleteEffect::None,
        );
        assert_eq!(
            classify_command_complete(b"DISCARD TEMP\0"),
            CommandCompleteEffect::None,
        );
        assert_eq!(
            classify_command_complete(b"DISCARD SEQUENCES\0"),
            CommandCompleteEffect::None,
        );
    }

    #[test]
    fn regular_command_tags_are_inert() {
        // A representative sample of data-plane tags. If any of these ever
        // start influencing cleanup tracking it will be a correctness bug.
        for tag in [
            &b"SELECT 1\0"[..],
            b"INSERT 0 1\0",
            b"UPDATE 5\0",
            b"DELETE 10\0",
            b"BEGIN\0",
            b"COMMIT\0",
            b"ROLLBACK\0",
            b"UNLISTEN\0",
            b"SAVEPOINT\0",
        ] {
            assert_eq!(
                classify_command_complete(tag),
                CommandCompleteEffect::None,
                "tag {:?} should not influence cleanup",
                std::str::from_utf8(tag).unwrap_or("<non-utf8>"),
            );
        }
    }

    #[test]
    fn length_only_matches_do_not_confuse_classifier() {
        // Both DECLARE CURSOR and DEALLOCATE ALL are 15 bytes long with the
        // trailing NUL; the classifier must dispatch on content, not length.
        assert_eq!(
            classify_command_complete(b"DECLARE CURSOR\0"),
            CommandCompleteEffect::ArmDeclare,
        );
        assert_eq!(
            classify_command_complete(b"DEALLOCATE ALL\0"),
            CommandCompleteEffect::DisarmPrepare,
        );
        // Same length as DEALLOCATE ALL but unrelated content — must be inert.
        assert_eq!(
            classify_command_complete(b"MADE UP TAG 01\0"),
            CommandCompleteEffect::None,
        );
    }

    #[test]
    fn empty_or_missing_nul_is_inert() {
        assert_eq!(classify_command_complete(b""), CommandCompleteEffect::None,);
        // Without the trailing NUL the length never matches the expected one.
        assert_eq!(
            classify_command_complete(b"SET"),
            CommandCompleteEffect::None,
        );
        assert_eq!(
            classify_command_complete(b"RESET"),
            CommandCompleteEffect::None,
        );
    }
}
