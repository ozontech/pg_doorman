mod core;
mod entrypoint;
mod error_handling;
mod protocol;
mod startup;
mod transaction;
mod util;
pub mod buffer_pool;

pub use core::Client;
pub use entrypoint::{client_entrypoint, client_entrypoint_too_many_clients_already};
pub use startup::startup_tls;
pub use util::{CLIENT_COUNTER, PREPARED_STATEMENT_COUNTER};
