//! Web UI and metrics endpoint configuration.

use std::path::PathBuf;

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

    /// Enable JWT-based SSO authentication on the web UI. When `true`,
    /// `sso_public_key_file` and `sso_audience` must also be set; missing
    /// values silently demote SSO to disabled (logged at error level) so
    /// the listener still serves Basic-only requests.
    #[serde(default)]
    pub sso_enabled: bool,

    /// URL of the external SSO proxy used by the SPA for the
    /// "Sign in via SSO" redirect. Server-side validation does not depend
    /// on this — backend only validates the JWT signature against
    /// `sso_public_key_file`.
    #[serde(default)]
    pub sso_proxy_url: Option<String>,

    /// Path to a PEM file containing the RSA public key paired with the
    /// SSO proxy's signing key.
    #[serde(default)]
    pub sso_public_key_file: Option<PathBuf>,

    /// Allowed values of the `aud` JWT claim. A token is accepted when
    /// its audience matches at least one entry in this list.
    #[serde(default)]
    pub sso_audience: Vec<String>,

    /// Allowed `preferred_username`/`sub` claims. `["*"]` (the default)
    /// accepts every valid JWT; a literal list restricts access to those
    /// usernames only.
    #[serde(default = "Web::default_sso_allowed_users")]
    pub sso_allowed_users: Vec<String>,

    /// CIDR ranges that the listener trusts to set
    /// `X-Forwarded-For` / `Forwarded`. When a request arrives from a
    /// peer in this list, the access log resolves the real client IP
    /// from the proxy header instead of using the proxy's TCP peer.
    /// Empty (the default) disables the override — the listener
    /// trusts only its own peer.
    #[serde(default)]
    pub trusted_proxies: Vec<ipnet::IpNet>,

    /// JWT claim name that carries the user's group memberships.
    /// Used together with `sso_admin_groups`: when the validated JWT
    /// has a value of this claim that intersects the admin groups
    /// list, the request resolves to the `Admin` role rather than
    /// `Sso`. Default `groups` matches Keycloak / Authelia / oauth2-
    /// proxy out of the box.
    #[serde(default = "Web::default_sso_groups_claim")]
    pub sso_groups_claim: String,

    /// Group names that map onto the `Admin` role. An SSO user whose
    /// JWT carries any of these group names in `sso_groups_claim`
    /// gets full admin access (including `POST /api/admin/*`).
    /// Empty (the default) keeps the SSO surface read-only — every
    /// SSO user resolves to `Sso`.
    #[serde(default)]
    pub sso_admin_groups: Vec<String>,

    /// Reject Bearer/cookie/query SSO credentials when the request did
    /// not arrive over HTTPS. The listener treats a request as secure
    /// only when its TCP peer is in `trusted_proxies` and the proxy
    /// forwarded `X-Forwarded-Proto: https`. Defaults to `false` so
    /// existing deployments where the SSO proxy terminates TLS on a
    /// different host (and reaches pg_doorman over a private network)
    /// keep working without configuration changes. Enable on multi-
    /// tenant networks where an attacker could observe the HTTP leg
    /// between the proxy and pg_doorman.
    #[serde(default)]
    pub sso_require_https: bool,
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
            sso_enabled: false,
            sso_proxy_url: None,
            sso_public_key_file: None,
            sso_audience: Vec::new(),
            sso_allowed_users: Self::default_sso_allowed_users(),
            trusted_proxies: Vec::new(),
            sso_groups_claim: Self::default_sso_groups_claim(),
            sso_admin_groups: Vec::new(),
            sso_require_https: false,
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

    /// `["*"]` — any valid JWT grants Sso role. Operators wanting to
    /// restrict to a known set of usernames replace this list explicitly.
    pub fn default_sso_allowed_users() -> Vec<String> {
        vec!["*".to_string()]
    }

    /// `groups` — the claim name used by Keycloak, oauth2-proxy, and
    /// Authelia. Operators using a non-standard SSO proxy override this.
    pub fn default_sso_groups_claim() -> String {
        "groups".to_string()
    }
}
