//! HTTP listener + path mux for the web subsystem.
//!
//! Routes:
//! - `GET /metrics`      → Prometheus exporter, no auth.
//! - `GET /api/version`  → version info, public.
//! - `GET /api/overview` → cluster overview, public.
//! - `GET /api/pools`    → pool list, public.
//! - `GET /api/*`        → route table in [`router`].
//! - `GET /` | `GET /assets/*` → SPA static assets.
//! - everything else → 404.
//!
//! Submodule layout:
//! - [`state`]    — reload-aware [`WebServerOptions`] backed by `ArcSwap`.
//! - [`wire`]     — request parser, response builder, gzip cache, header helpers.
//! - [`http`]     — keep-alive driven HTTP/1.1 connection handler.
//! - [`router`]   — path dispatch and admin-only gating.
//! - [`listener`] — TCP bind and accept loop.

mod http;
mod listener;
mod router;
pub(crate) mod state;
pub(crate) mod wire;

#[cfg(test)]
mod tests;

pub use listener::{bind_web_listener, serve_on};
pub use state::{refresh_options_from_config, WebServerOptions};
pub(crate) use wire::Response;
