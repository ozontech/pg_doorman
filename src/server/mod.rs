//! `crate::server` module (backend PostgreSQL connection and protocol handling).

pub(crate) mod authentication;
pub(crate) mod cleanup;
pub(crate) mod parameters;
pub(crate) mod prepared_statements;
pub(crate) mod protocol_io;
pub(crate) mod startup_cancel;
pub(crate) mod startup_error;
pub(crate) mod stream;

mod prepared_statement_cache;
mod server_backend;

pub use parameters::ServerParameters;
pub use prepared_statement_cache::{
    anon_len, anon_snapshot, gc_sweep_anon, gc_sweep_named, intern_query, named_len,
    named_snapshot, now_monotonic_ms, reset_interners_force, AnonEntry, CacheEntryKind, GcStats,
    NamedEntry, PreparedStatementCache,
};

#[cfg(test)]
pub use prepared_statement_cache::{
    anon_entry_for_test, named_entry_for_test, reset_interners_for_test,
};
pub use server_backend::Server;
pub use stream::StreamInner;
