//! REST API routes mounted under `/api/`.
//!
//! Phase 3a wires only `/api/version`, `/api/overview`, `/api/pools`.
//! Subsequent phases add `/api/clients`, `/api/servers`, top-N, etc.

pub mod collect;
pub mod dto;

pub(crate) mod overview;
pub(crate) mod pools;
pub(crate) mod version;
