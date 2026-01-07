//! `crate::server` module (backend PostgreSQL connection and protocol handling).

pub(crate) mod cleanup;
pub(crate) mod parameters;
pub(crate) mod prepared_statements;
pub(crate) mod protocol_io;
pub(crate) mod startup_cancel;
pub(crate) mod stream;

#[allow(clippy::module_inception)]
mod server;

pub use parameters::ServerParameters;
pub use server::Server;
pub use stream::StreamInner;
