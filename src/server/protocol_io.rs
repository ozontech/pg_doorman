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
use tokio::time::timeout;

use crate::config::get_config;
use crate::errors::Error;
use crate::errors::Error::MaxMessageSize;
use crate::messages::PgErrorMsg;
use crate::messages::MAX_MESSAGE_SIZE;
use crate::messages::{
    proxy_copy_data, proxy_copy_data_with_timeout, read_message_data, read_message_header,
    write_all_flush, BytesMutReader,
};

use super::parameters::ServerParameters;
use super::server_backend::Server;

// PostgreSQL CommandComplete message payloads for tracking session state changes
/// CommandComplete payload for SET statements (requires RESET ALL cleanup)
const COMMAND_COMPLETE_BY_SET: &[u8; 4] = b"SET\0";
/// CommandComplete payload for DECLARE CURSOR statements (requires CLOSE ALL cleanup)
const COMMAND_COMPLETE_BY_DECLARE: &[u8; 15] = b"DECLARE CURSOR\0";
/// CommandComplete payload for SAVEPOINT statements (enables savepoint mode)
const COMMAND_SAVEPOINT: &[u8; 10] = b"SAVEPOINT\0";
/// CommandComplete payload for DEALLOCATE ALL (clears prepared statement cache)
const COMMAND_COMPLETE_BY_DEALLOCATE_ALL: &[u8; 15] = b"DEALLOCATE ALL\0";
/// CommandComplete payload for DISCARD ALL (clears prepared statement cache)
const COMMAND_COMPLETE_BY_DISCARD_ALL: &[u8; 12] = b"DISCARD ALL\0";

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
            server.mark_bad("flush timeout error");
            error!(
                "Flush timeout for server {} (database: {}, user: {}). Operation took longer than the configured timeout: {}",
                server.address.host,
                server.address.database,
                server.address.username,
                err
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
                "Terminating connection to server {} (database: {}, user: {}) due to error: {}",
                server.address.host, server.address.database, server.address.username, err
            );
            server.mark_bad("flush to server error");
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
        Duration::from_millis(get_config().general.proxy_copy_data_timeout),
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
            server.is_aborted = false;
            server.in_transaction = true;
        }

        // 'I' - Idle (not in transaction)
        'I' => {
            server.is_aborted = false;
            server.in_transaction = false;
        }

        // 'E' - In failed transaction block (requires ROLLBACK)
        'E' => {
            server.is_aborted = true;
            server.in_transaction = true;
            if let Ok(msg) = PgErrorMsg::parse(message) {
                error!(
                    "Transaction error on server {} (database: {}, user: {}). Transaction was rolled back. Details: [Severity: {}, Code: {}, Message: \"{}\", Hint: \"{}\", Position: {}]",
                    server.address.host,
                    server.address.database,
                    server.address.username,
                    msg.severity,
                    msg.code,
                    msg.message,
                    msg.hint.as_deref().unwrap_or("none"),
                    msg.position.unwrap_or(0)
                );
            } else {
                error!(
                    "Transaction error on server {} (database: {}, user: {}). Transaction was rolled back. Could not parse error details.",
                    server.address.host,
                    server.address.database,
                    server.address.username
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
                format!(
                    "Protocol sync error: unknown transaction state '{transaction_state}'"
                )
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
        let transaction_status = if server.in_transaction {
            "in active transaction"
        } else {
            "not in transaction"
        };
        let copy_mode_status = if server.in_copy_mode {
            "in COPY mode"
        } else {
            "not in COPY mode"
        };

        error!(
            "PostgreSQL server error from {} (database: {}, user: {}). Status: [{}, {}]. Error details: [Severity: {}, Code: {}, Message: \"{}\", Hint: \"{}\", Detail: \"{}\", Position: {}]",
            server.address.host,
            server.address.database,
            server.address.username,
            transaction_status,
            copy_mode_status,
            msg.severity,
            msg.code,
            msg.message,
            msg.hint.as_deref().unwrap_or("none"),
            msg.detail.as_deref().unwrap_or("none"),
            msg.position.unwrap_or(0)
        );
    } else {
        error!(
            "PostgreSQL server error from {} (database: {}, user: {}). Could not parse error details.",
            server.address.host,
            server.address.database,
            server.address.username
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
        server.mark_bad("PostgreSQL error in asynchronous operation mode");
    }
}

/// Handles CommandComplete ('C') message - indicates successful completion of a command.
/// Tracks commands that require cleanup (SET, DECLARE, etc.) and updates server state.
fn handle_command_complete(server: &mut Server, message: &BytesMut) {
    // Exit COPY mode if we were in it
    if server.in_copy_mode {
        server.in_copy_mode = false;
    }
    
    // Check for commands that require cleanup at connection checkin
    if message.len() == 4 && message.to_vec().eq(COMMAND_COMPLETE_BY_SET) {
        server.cleanup_state.needs_cleanup_set = true;
    }
    if message.len() == 10 && message.to_vec().eq(COMMAND_SAVEPOINT) {
        server.use_savepoint = true;
    }
    if message.len() == 15 && message.to_vec().eq(COMMAND_COMPLETE_BY_DECLARE) {
        server.cleanup_state.needs_cleanup_declare = true;
    }
    if message.len() == 12 && message.to_vec().eq(COMMAND_COMPLETE_BY_DISCARD_ALL) {
        server.registering_prepared_statement.clear();
        if server.prepared_statement_cache.is_some() {
            warn!("Cleanup server {server} prepared statements cache (DISCARD ALL)");
            server.prepared_statement_cache.as_mut().unwrap().clear();
        }
    }
    if message.len() == 15 && message.to_vec().eq(COMMAND_COMPLETE_BY_DEALLOCATE_ALL) {
        server.registering_prepared_statement.clear();
        if server.prepared_statement_cache.is_some() {
            warn!("Cleanup server {server} prepared statements cache (DEALLOCATE ALL)");
            server.prepared_statement_cache.as_mut().unwrap().clear();
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
        client_server_parameters.set_param(key.clone(), value.clone(), false);
        if server.log_client_parameter_status_changes {
            info!("Server {server}: client parameter status change: {key} = {value}")
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
    loop {
        server.stats.wait_reading();

        // In async mode, use a short timeout to avoid blocking when no more data available
        let (code_u8, message_len) = if server.is_async() {
            match tokio::time::timeout(
                Duration::from_millis(100),
                read_message_header(&mut server.stream),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    // Timeout - no more data available in async mode
                    server.data_available = false;
                    break;
                }
            }
        } else {
            read_message_header(&mut server.stream).await?
        };
        // Handle large DataRow messages that exceed max_message_size
        if server.max_message_size > 0
            && message_len > server.max_message_size
            && code_u8 as char == 'D'
        {
            return handle_large_data_row(server, &mut client_stream, code_u8, message_len).await;
        }
        
        // Handle large CopyData messages that exceed max_message_size
        if server.max_message_size > 0
            && message_len > server.max_message_size
            && code_u8 as char == 'd'
        {
            return handle_large_copy_data(server, &mut client_stream, code_u8, message_len).await;
        }

        if message_len > MAX_MESSAGE_SIZE {
            error!(
                "Message size limit exceeded for server connection to {} (database: {}, user: {}). Received message size: {} bytes, maximum allowed: {} bytes. Connection will be terminated.",
                server.address.host,
                server.address.database,
                server.address.username,
                message_len,
                MAX_MESSAGE_SIZE
            );
            server.mark_bad(
                format!(
                    "Message size limit exceeded: {message_len} bytes (max: {MAX_MESSAGE_SIZE} bytes)"
                )
                .as_str(),
            );
            return Err(MaxMessageSize);
        }

        let mut message = match read_message_data(&mut server.stream, code_u8, message_len).await {
            Ok(message) => {
                server.stats.wait_idle();
                message
            }
            Err(err) => {
                error!(
                    "Terminating server connection to {} (database: {}, user: {}) while reading message data. Error details: {err}",
                    server.address.host,
                    server.address.database,
                    server.address.username
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
            }

            // CommandComplete - command executed successfully
            'C' => {
                handle_command_complete(server, &message);
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
                if server.buffer.len() >= 8196 {
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
                if server.buffer.len() >= 8196 {
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
                    server.data_available = false;
                }
            }

            // BindComplete
            // Response to Bind message in extended query protocol
            '2' => {
                if server.is_async() {
                    server.data_available = false;
                }
            }

            // CloseComplete
            // Response to Close message in extended query protocol
            '3' => {
                if server.is_async() {
                    server.data_available = false;
                }
            }

            // ParameterDescription
            // Response to Describe message for a statement
            't' => {
                if server.is_async() {
                    server.data_available = false;
                }
            }

            // PortalSuspended
            // Indicates that Execute completed but portal still has rows
            's' => {
                if server.is_async() {
                    server.data_available = false;
                }
            }

            // NoData
            // Response to Describe when statement/portal produces no rows
            // https://www.postgresql.org/docs/current/protocol-flow.html
            'n' => {
                if server.is_async() {
                    server.data_available = false;
                }
            }

            // Anything else, e.g. errors, notices, etc.
            // Keep buffering until ReadyForQuery shows up.
            _ => (),
        };
    }

    let bytes = server.buffer.clone();

    // Keep track of how much data we got from the server for stats.
    server.stats.data_received(bytes.len());

    // Clear the buffer for next query.
    if server.buffer.len() > 8196 {
        server.buffer = BytesMut::with_capacity(8196);
    } else {
        // Clear the buffer for next query.
        server.buffer.clear();
    }

    // Successfully received data from server
    server.last_activity = SystemTime::now();

    // Pass the data back to the client.
    Ok(bytes)
}
