//! Reload-aware listener options. Every request reads the current value
//! through [`current_options`]; admin-protocol `RELOAD` and the REST
//! `/api/admin/reload` endpoint update the global config and then call
//! [`refresh_options_from_config`] to swap the slot atomically.

use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;

use crate::config::Config;

/// Runtime state needed by the mux on every request.
#[derive(Clone)]
pub struct WebServerOptions {
    /// `true` when `[web].ui = true` AND admin_password is non-default.
    /// When `false`, the listener serves only `/metrics`; everything else → 404.
    pub ui_active: bool,
    /// `[web].ui_anonymous` — gates the public `/api/*` endpoints when
    /// `ui_active`. The SPA shell (HTML/CSS/JS/font/svg) is always served
    /// anonymously so a hard refresh of a deep link does not trigger a
    /// browser-native basic-auth prompt on top of the React `AuthGate`.
    pub ui_anonymous: bool,
    pub admin_username: String,
    pub admin_password: String,
    /// SSO runtime (RS256 decoding key, validation config, allowlist).
    /// `None` when `[web].sso_enabled = false` or when the public-key
    /// file failed to load. Threaded into `classify` so the JWT branch
    /// can validate Bearer/cookie/query tokens.
    pub sso: Option<std::sync::Arc<crate::web::sso::SsoRuntime>>,
}

impl WebServerOptions {
    /// Build the request-time options from a config snapshot. `ui_active`
    /// is gated on a non-default admin password — `web.ui = true` paired
    /// with an empty/`"admin"` password is silently demoted to "metrics
    /// only", matching the explicit warning the startup path logs in
    /// `app::server::run_server`.
    pub fn from_config(cfg: &Config) -> Self {
        let admin_default =
            cfg.general.admin_password.is_empty() || cfg.general.admin_password == "admin";
        WebServerOptions {
            ui_active: cfg.web.ui && !admin_default,
            ui_anonymous: cfg.web.ui_anonymous,
            admin_username: cfg.general.admin_username.clone(),
            admin_password: cfg.general.admin_password.clone(),
            // Populated in a later commit. Leaving `None` here keeps the
            // listener Basic-only until SSO loading lands.
            sso: None,
        }
    }
}

/// Reload-aware options snapshot used by every request. Installed once on
/// `start_web_server`, swapped atomically when the admin protocol or the
/// REST `/api/admin/reload` endpoint replaces the global config. Without
/// this, `RELOAD` would update `/api/config` but the listener would keep
/// authenticating against the old password and ignoring `[web].ui_anonymous`
/// changes until the next process restart.
static WEB_OPTIONS: OnceLock<ArcSwap<WebServerOptions>> = OnceLock::new();

pub(super) fn install_options(opts: Arc<WebServerOptions>) {
    if let Some(swap) = WEB_OPTIONS.get() {
        swap.store(opts);
    } else {
        let _ = WEB_OPTIONS.set(ArcSwap::from(opts));
    }
}

pub(super) fn current_options() -> Arc<WebServerOptions> {
    WEB_OPTIONS
        .get()
        .map(|swap| swap.load_full())
        .unwrap_or_else(|| {
            // Fallback for code paths that read options before the listener
            // started. Recomputes from the live config so behavior is at
            // least defined; `start_web_server` will replace it on bind.
            Arc::new(WebServerOptions::from_config(&crate::config::get_config()))
        })
}

/// Re-derive the listener's runtime options from the current global config.
/// Called by every code path that updates the global `Config` (admin
/// protocol `RELOAD`, REST `/api/admin/reload`). Idempotent.
pub fn refresh_options_from_config() {
    let cfg = crate::config::get_config();
    install_options(Arc::new(WebServerOptions::from_config(&cfg)));
}
