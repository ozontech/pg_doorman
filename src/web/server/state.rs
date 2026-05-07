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
    /// Human-readable reason `sso` is `None` despite
    /// `[web].sso_enabled = true`. Returned through `/api/auth/config`
    /// so the SPA can show "SSO is configured but not loaded:
    /// <reason>" instead of silently falling back to Basic-only.
    pub sso_config_error: Option<String>,
    /// CIDR ranges trusted to set `X-Forwarded-For` / `Forwarded`. When
    /// the request peer falls in this list, the access log resolves
    /// the real client IP from the proxy header.
    pub trusted_proxies: Vec<ipnet::IpNet>,
    /// `true` when `[web].sso_admin_groups` is non-empty. Surfaced on
    /// `/api/auth/config` so the SPA's sign-in modal stops promising
    /// "SSO grants read-only access" when the operator may actually
    /// land in Admin via group membership.
    pub sso_admin_groups_configured: bool,
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
        let (sso, sso_config_error) = if cfg.web.sso_enabled {
            match build_sso_runtime(&cfg.web) {
                Ok(rt) => (Some(rt), None),
                Err(reason) => (None, Some(reason)),
            }
        } else {
            (None, None)
        };
        WebServerOptions {
            ui_active: cfg.web.ui && !admin_default,
            ui_anonymous: cfg.web.ui_anonymous,
            admin_username: cfg.general.admin_username.clone(),
            admin_password: cfg.general.admin_password.clone(),
            sso,
            sso_config_error,
            trusted_proxies: cfg.web.trusted_proxies.clone(),
            sso_admin_groups_configured: !cfg.web.sso_admin_groups.is_empty(),
        }
    }
}

/// Build the SSO runtime from `[web].sso_*`. Missing or invalid config
/// returns an error string instead of aborting the listener, so a typo
/// in the SSO section never knocks the operator console offline. The
/// caller logs the error and forwards it through `/api/auth/config`.
fn build_sso_runtime(
    web: &crate::config::web::Web,
) -> Result<Arc<crate::web::sso::SsoRuntime>, String> {
    use crate::web::sso::{AdminBridge, AllowedUsers, SsoRuntime};

    let Some(path) = web.sso_public_key_file.as_ref() else {
        let msg = "[web].sso_enabled=true but sso_public_key_file is missing".to_string();
        log::error!("{msg}; SSO disabled for this run");
        return Err(msg);
    };
    if web.sso_audience.is_empty() {
        let msg = "[web].sso_enabled=true but sso_audience is empty".to_string();
        log::error!("{msg}; SSO disabled for this run");
        return Err(msg);
    }
    let allowed = AllowedUsers::from_config(&web.sso_allowed_users);
    let admin_bridge = AdminBridge::from_config(&web.sso_groups_claim, &web.sso_admin_groups);
    match SsoRuntime::from_pem_file(
        path,
        &web.sso_audience,
        allowed,
        web.sso_proxy_url.clone(),
        admin_bridge,
    ) {
        Ok(rt) => Ok(Arc::new(rt)),
        Err(e) => {
            let msg = format!("SSO public key load failed: {e}");
            log::error!("[web] {msg}");
            Err(msg)
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
    crate::web::metrics::WEB_SSO_ENABLED.set(if opts.sso.is_some() { 1 } else { 0 });
    crate::web::metrics::WEB_SSO_CONFIG_ERROR.set(if opts.sso_config_error.is_some() {
        1
    } else {
        0
    });
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
            // started. Recomputes from the live config so the call has a
            // deterministic answer; `start_web_server` overwrites the slot
            // when it binds.
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
