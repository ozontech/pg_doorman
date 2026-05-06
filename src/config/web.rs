//! Web UI and metrics endpoint configuration.

use serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Web {
    #[serde(default = "Web::default_host")]
    pub host: String,
    #[serde(default = "Web::default_port")]
    pub port: u16,
    #[serde(default = "Web::default_enabled")]
    pub enabled: bool,
    #[serde(default = "Web::default_ui")]
    pub ui: bool,
    #[serde(default = "Web::default_ui_anonymous")]
    pub ui_anonymous: bool,
    #[serde(default = "Web::default_log_tap_max_entries")]
    pub log_tap_max_entries: u32,
}

impl Web {
    pub fn empty() -> Web {
        Web {
            host: Self::default_host(),
            port: Self::default_port(),
            enabled: Self::default_enabled(),
            ui: Self::default_ui(),
            ui_anonymous: Self::default_ui_anonymous(),
            log_tap_max_entries: Self::default_log_tap_max_entries(),
        }
    }

    /// Default host/port match the legacy prometheus listener so existing deployments
    /// and Grafana scrape configs keep working without changes.
    pub fn default_host() -> String {
        "0.0.0.0".to_string()
    }

    pub fn default_port() -> u16 {
        9127
    }

    /// Whole HTTP listener is opt-in. Matches the legacy `prometheus.enabled = false` default.
    pub fn default_enabled() -> bool {
        false
    }

    /// Web UI is opt-in. Listener exists for /metrics by default; SPA and /api/* are gated behind this flag
    /// plus a non-default admin_password.
    pub fn default_ui() -> bool {
        false
    }

    /// Public read-only routes accessible without auth. Defaults to `false` —
    /// `/api/clients` exposes per-client peer addresses and application names,
    /// `/api/top/queries` exposes statement text, and other endpoints leak
    /// pool topology that is more sensitive than the aggregate `/metrics`
    /// counters. The operator who wants public read-only access flips this
    /// flag deliberately.
    pub fn default_ui_anonymous() -> bool {
        false
    }

    /// Capacity of in-memory log ring buffer (entries, not bytes). 8192 ≈ 1.5–2 minutes of history
    /// at info-level under 5kTPS, ~2 MB RSS at 250 bytes/entry. Smaller values (e.g., 1024) lose
    /// live-tail usefulness on a hot pooler; larger values waste RSS for the default.
    pub fn default_log_tap_max_entries() -> u32 {
        8192
    }
}
