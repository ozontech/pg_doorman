//! REST API routes mounted under `/api/`.
//!
//! Phase 3a wires only `/api/version`, `/api/overview`, `/api/pools`.
//! Subsequent phases add `/api/clients`, `/api/servers`, top-N, etc.

pub mod collect;
pub mod dto;

pub(crate) mod apps;
pub(crate) mod auth_query;
pub(crate) mod clients;
pub(crate) mod config;
pub(crate) mod connections;
pub(crate) mod databases;
pub(crate) mod interner;
pub(crate) mod interner_top;
pub(crate) mod log_level;
pub(crate) mod overview;
pub(crate) mod pool_coordinator;
pub(crate) mod pool_scaling;
pub(crate) mod pools;
pub(crate) mod prepared;
pub(crate) mod prepared_text;
pub(crate) mod query;
pub(crate) mod servers;
pub(crate) mod sockets;
pub(crate) mod stats;
pub(crate) mod top_clients;
pub(crate) mod users;
pub(crate) mod version;
