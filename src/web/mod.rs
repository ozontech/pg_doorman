//! Web subsystem: Prometheus metrics endpoint, future REST API for the UI,
//! authentication, log tap, and SPA static assets.
//!
//! Phase 1 wires only the metrics submodule (the former `crate::web::metrics`).
//! Auth, routes, log_tap, and static_assets are added in subsequent phases.

pub mod metrics;
