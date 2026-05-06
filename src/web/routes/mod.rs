//! REST API routes mounted under `/api/`.
//!
//! Phase 3a wires only `/api/version`, `/api/overview`, `/api/pools`.
//! Subsequent phases add `/api/clients`, `/api/servers`, top-N, etc.

pub mod collect;
pub mod dto;

pub(crate) mod clients;
pub(crate) mod connections;
pub(crate) mod databases;
pub(crate) mod overview;
pub(crate) mod pools;
pub(crate) mod query;
pub(crate) mod servers;
pub(crate) mod stats;
pub(crate) mod users;
pub(crate) mod version;
