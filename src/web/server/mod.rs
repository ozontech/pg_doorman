//! HTTP listener + path mux for the web subsystem.
//!
//! Routes:
//! - `GET /metrics`      → Prometheus exporter, no auth.
//! - `GET /api/version`  → version info, public.
//! - `GET /api/overview` → cluster overview, public.
//! - `GET /api/pools`    → pool list, public.
//! - `GET /api/*`        → other endpoints return 501 until wired in later phases.
//! - `GET /` | `GET /assets/*` → SPA placeholder, returns 404 (filled in phase 7).
//! - everything else → 404.
//!
//! Submodule layout (codex Arch P2#5 split — the original single-file
//! `server.rs` mixed listener lifecycle, HTTP parsing, auth policy, routing,
//! and response serialization in ~1300 lines):
//! - [`state`]    — reload-aware [`WebServerOptions`] backed by `ArcSwap`.
//! - [`wire`]     — request parser, response builder, gzip cache, header helpers.
//! - [`http`]     — keep-alive driven HTTP/1.1 connection handler.
//! - [`router`]   — path dispatch and admin-only gating.
//! - [`listener`] — TCP bind and accept loop.

mod http;
mod listener;
mod router;
mod state;
mod wire;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use listener::start_web_server;
pub use listener::{bind_web_listener, serve_on};
pub use state::{refresh_options_from_config, WebServerOptions};
pub(crate) use wire::Response;
