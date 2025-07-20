// Helper functions to send one-off protocol messages and handle TcpStream (TCP socket).

// Standard library imports
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

// External crate imports
use once_cell::sync::Lazy;

// Declare submodules
pub mod config_socket;
pub mod error;
pub mod extended;
pub mod protocol;
pub mod socket;
pub mod types;

// Re-export public items
pub use config_socket::{configure_tcp_socket, configure_unix_socket};
pub use error::{set_messages_right_place, PgErrorMsg};
pub use extended::{close_complete, Bind, Close, Describe, ExtendedProtocolData, Parse};
pub use protocol::{
    check_query_response, command_complete, data_row, data_row_nullable, deallocate_response,
    error_message, error_response, error_response_terminal, flush, md5_challenge,
    md5_hash_password, md5_hash_second_pass, md5_password, md5_password_with_hash, notify,
    parse_complete, parse_params, parse_startup, plain_password_challenge, read_password,
    ready_for_query, scram_server_response, scram_start_challenge, server_parameter_message,
    simple_query, ssl_request, startup, sync, wrong_password,
};
pub use socket::{
    proxy_copy_data, proxy_copy_data_with_timeout, read_message, read_message_data,
    read_message_header, write_all, write_all_flush, write_all_half,
};
pub use types::{vec_to_string, BytesMutReader, DataType};

// Re-export constants
pub use crate::constants::{AUTHENTICATION_CLEAR_PASSWORD, MESSAGE_TERMINATOR, SCRAM_SHA_256};

// Constants
pub const MAX_MESSAGE_SIZE: i32 = 256 * 1024 * 1024;

// Global state
pub static CURRENT_MEMORY: Lazy<Arc<AtomicI64>> = Lazy::new(|| Arc::new(AtomicI64::new(0)));

// Tests
#[cfg(test)]
mod tests;
