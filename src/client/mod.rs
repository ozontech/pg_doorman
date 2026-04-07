mod batch_handling;
pub mod buffer_pool;
mod core;
mod entrypoint;
mod error_handling;
#[cfg(unix)]
pub mod migration;
mod protocol;
mod startup;
mod transaction;
mod util;

pub use core::Client;
pub use entrypoint::{
    client_entrypoint, client_entrypoint_too_many_clients_already,
    client_entrypoint_too_many_clients_already_unix, client_entrypoint_unix, ClientSessionInfo,
};
pub use startup::startup_tls;
pub use util::PREPARED_STATEMENT_COUNTER;
