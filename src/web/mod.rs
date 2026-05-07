//! Web subsystem: Prometheus metrics endpoint, future REST API for the UI,
//! authentication, log tap, and SPA static assets.
//!
//! Phase 1 wires only the metrics submodule (the former `crate::web::metrics`).
//! Auth, routes, log_tap, and static_assets are added in subsequent phases.

pub mod access_log;
pub mod auth;
pub mod log_tap;
pub mod metrics;
pub mod routes;
pub mod server;
pub mod sso;
pub mod static_assets;

#[cfg(test)]
pub(crate) use server::start_web_server;
pub use server::{bind_web_listener, refresh_options_from_config, serve_on, WebServerOptions};

#[cfg(test)]
mod tests;
