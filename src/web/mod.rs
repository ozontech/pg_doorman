//! Web subsystem: Prometheus metrics, REST API routes, authentication,
//! log tap, and SPA static assets.

pub mod access_log;
pub mod auth;
pub mod log_tap;
pub mod metrics;
pub mod peer;
pub mod routes;
pub mod server;
pub mod sso;
pub mod static_assets;

pub use server::{bind_web_listener, refresh_options_from_config, serve_on, WebServerOptions};

#[cfg(test)]
mod tests;
