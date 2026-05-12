//! Authentication classifier for the web mux.
//!
//! Recognises both `Basic` (Admin role) and `Bearer` JWTs (Sso role),
//! plus query/cookie SSO sources, and produces an `AuthOutcome` that the
//! router translates into 200 / 401 / 403.

use base64::Engine;
use subtle::ConstantTimeEq;

/// Logical role for a request. Ordered: `Admin > Sso > Anonymous`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Anonymous,
    Sso,
    Admin,
}

/// How the operator presented their credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    Basic,
    Sso,
}

/// Identity attached to a successfully authenticated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIdentity {
    pub username: String,
    pub source: AuthSource,
}

/// Authentication outcome for an inbound request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    /// No credentials were presented.
    Anonymous,
    /// A valid JWT was presented; read-only role.
    Sso(AuthIdentity),
    /// Valid Basic credentials were presented; full-access role.
    Admin(AuthIdentity),
    /// At least one credential was presented and all failed.
    Rejected,
}

impl AuthOutcome {
    pub fn role(&self) -> Role {
        match self {
            AuthOutcome::Admin(_) => Role::Admin,
            AuthOutcome::Sso(_) => Role::Sso,
            AuthOutcome::Anonymous | AuthOutcome::Rejected => Role::Anonymous,
        }
    }

    pub fn identity(&self) -> Option<&AuthIdentity> {
        match self {
            AuthOutcome::Admin(id) | AuthOutcome::Sso(id) => Some(id),
            _ => None,
        }
    }
}

/// Whether the listener should accept SSO credentials presented over a
/// plain-HTTP request. `request_is_secure` is the listener's verdict
/// after combining the TCP peer with `X-Forwarded-Proto` (see
/// [`crate::web::peer::request_is_secure`]); `require_https` is the
/// operator's `[web].sso_require_https` knob.
///
/// Default policy (`require_https=false`) keeps backward compatibility:
/// every deployment where a TLS-terminating proxy reaches pg_doorman
/// over a private HTTP leg keeps working without configuration changes.
/// Opt-in `require_https=true` rejects SSO credentials on plain HTTP so
/// the JWT cannot leak between the proxy and pg_doorman.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SsoTransportPolicy {
    pub request_is_secure: bool,
    pub require_https: bool,
}

impl SsoTransportPolicy {
    /// Permits SSO credentials when the operator has not opted in to
    /// HTTPS-only SSO, or when the request actually arrived over a
    /// trusted HTTPS hop.
    fn permits_sso(self) -> bool {
        !self.require_https || self.request_is_secure
    }
}

/// Classify an inbound HTTP request into an `AuthOutcome`. Recognises:
///
/// - `Authorization: Basic <b64(user:pass)>` against the admin
///   credentials (constant-time compare; success → `Admin`).
/// - `Authorization: Bearer <jwt>`, `?token=<jwt>` and
///   `Cookie: sso_access_token=<jwt>` validated against `sso` (success →
///   `Sso`).
///
/// Order of preference: Basic > Bearer header > query > cookie. Basic
/// wins regardless of the SSO state: a correct admin password trumps
/// every SSO token. A broken Basic does not block a valid SSO token;
/// the function falls through to the SSO sources in order.
///
/// The Basic comparison runs in constant time relative to the configured
/// credentials to deny timing oracles. Both username and password legs
/// are checked together without short-circuit (see the `&` operator
/// inside the implementation).
///
/// `sso_transport` decides whether the SSO branches run at all. When
/// the operator set `[web].sso_require_https = true` and the request
/// did not arrive over a trusted HTTPS hop, every SSO source is
/// skipped and the function falls through to either `Rejected` (an
/// SSO credential was attempted) or `Anonymous` (no credentials at
/// all). Basic credentials are unaffected — they are scheme-bound by
/// `Authorization: Basic` and live or die on the constant-time
/// compare above, regardless of transport.
pub fn classify(
    authorization_header: Option<&str>,
    cookie_header: Option<&str>,
    query_token: Option<&str>,
    admin_username: &str,
    admin_password: &str,
    sso: Option<&crate::web::sso::SsoRuntime>,
    sso_transport: SsoTransportPolicy,
) -> AuthOutcome {
    let mut tried = false;

    // Step 1: Basic wins outright. A correct admin password trumps any
    // SSO token.
    if let Some(header) = authorization_header {
        if let Some(b64) = header.strip_prefix("Basic ") {
            tried = true;
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
                if let Ok(decoded_str) = std::str::from_utf8(&decoded) {
                    if let Some((user, pass)) = decoded_str.split_once(':') {
                        // `&` instead of `&&`: avoids short-circuit, both
                        // legs always evaluated so timing depends only on
                        // configured credential lengths.
                        let matches = bool::from(user.as_bytes().ct_eq(admin_username.as_bytes()))
                            & bool::from(pass.as_bytes().ct_eq(admin_password.as_bytes()));
                        if matches {
                            return AuthOutcome::Admin(AuthIdentity {
                                username: admin_username.to_string(),
                                source: AuthSource::Basic,
                            });
                        }
                    }
                }
            }
        }
    }

    // An `Authorization: Bearer ...` is an explicit credential attempt.
    // Even when SSO is disabled here, we treat it as `tried` so the caller
    // gets `Rejected` (and a 401) rather than silent `Anonymous`.
    let bearer_token = authorization_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.trim());
    if bearer_token.is_some() {
        tried = true;
    }

    // Steps 2-3: SSO sources, in priority order. Cookie and query are
    // less explicit than the Authorization header — they are common
    // ambient state from a reverse-proxy paving cookies on a shared
    // domain, or a stale link with `?token=`. We only count them as a
    // credential attempt when SSO is actually configured to consume
    // them; otherwise an Anonymous request to a public endpoint that
    // happens to carry such a cookie should still pass.
    //
    // Transport gate: when `sso_require_https` is on and the request
    // did not arrive over a trusted HTTPS hop, skip every SSO branch
    // and record one telemetry sample per blocked attempt. A request
    // that carried only `Authorization: Bearer …` still sets
    // `tried = true` above, so the caller falls through to a 401 —
    // important so an operator chasing a misconfigured proxy gets a
    // real failure instead of silent Anonymous behaviour.
    if let Some(rt) = sso {
        if sso_transport.permits_sso() {
            if let Some(token) = bearer_token {
                if let Ok(id) = rt.validate(token) {
                    return sso_outcome(id);
                }
            }
            if let Some(token) = query_token {
                tried = true;
                if let Ok(id) = rt.validate(token) {
                    return sso_outcome(id);
                }
            }
            if let Some(token) = cookie_header.and_then(find_sso_cookie) {
                tried = true;
                if let Ok(id) = rt.validate(token) {
                    return sso_outcome(id);
                }
            }
        } else {
            let presented = bearer_token.is_some()
                || query_token.is_some()
                || cookie_header.and_then(find_sso_cookie).is_some();
            if presented {
                tried = true;
                crate::web::metrics::WEB_SSO_VALIDATION_ERRORS
                    .with_label_values(&["insecure_transport"])
                    .inc();
            }
        }
    }

    if tried {
        AuthOutcome::Rejected
    } else {
        AuthOutcome::Anonymous
    }
}

/// Build the right `AuthOutcome` for a validated SSO identity. When
/// the JWT carried an admin group claim, the user gets the `Admin`
/// role; otherwise read-only `Sso`. The `source` stays `Sso` either
/// way so the access log and `/api/auth/config` can still tell SSO-
/// admins apart from Basic admins.
fn sso_outcome(id: crate::web::sso::ValidatedIdentity) -> AuthOutcome {
    let identity = AuthIdentity {
        username: id.username,
        source: AuthSource::Sso,
    };
    if id.is_admin {
        AuthOutcome::Admin(identity)
    } else {
        AuthOutcome::Sso(identity)
    }
}

/// Pull the value of `sso_access_token` out of a raw `Cookie:` header
/// value. Per RFC 6265 cookies in the same header are separated by `; `
/// (or `;`).
fn find_sso_cookie(cookie_header: &str) -> Option<&str> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(token) = part.strip_prefix("sso_access_token=") {
            return Some(token);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::sso::test_helpers::{mint_jwt, ClaimsBuilder};
    use crate::web::sso::{test_keys, AllowedUsers, SsoRuntime};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    fn admin(name: &str) -> AuthOutcome {
        AuthOutcome::Admin(AuthIdentity {
            username: name.to_string(),
            source: AuthSource::Basic,
        })
    }

    fn mint(exp_offset: i64, name: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        mint_jwt(&ClaimsBuilder {
            preferred_username: Some(name),
            sub: None,
            aud: serde_json::json!("pg_doorman"),
            exp: now + exp_offset,
        })
    }

    fn sso_rt(allowed: AllowedUsers) -> SsoRuntime {
        SsoRuntime::from_pem_bytes(
            test_keys::PUBLIC_PEM.as_bytes(),
            &["pg_doorman".to_string()],
            allowed,
            None,
            crate::web::sso::AdminBridge::default(),
        )
        .unwrap()
    }

    fn permissive_transport() -> SsoTransportPolicy {
        SsoTransportPolicy {
            request_is_secure: false,
            require_https: false,
        }
    }

    fn https_only_transport(is_secure: bool) -> SsoTransportPolicy {
        SsoTransportPolicy {
            request_is_secure: is_secure,
            require_https: true,
        }
    }

    fn classify_default(
        auth: Option<&str>,
        cookie: Option<&str>,
        query: Option<&str>,
        admin_user: &str,
        admin_pass: &str,
        sso: Option<&SsoRuntime>,
    ) -> AuthOutcome {
        classify(
            auth,
            cookie,
            query,
            admin_user,
            admin_pass,
            sso,
            permissive_transport(),
        )
    }

    #[test]
    fn anonymous_when_header_missing() {
        assert_eq!(
            classify_default(None, None, None, "admin", "secret", None),
            AuthOutcome::Anonymous
        );
    }

    #[test]
    fn admin_when_credentials_match() {
        let header = format!("Basic {}", b64("admin:secret"));
        assert_eq!(
            classify_default(Some(&header), None, None, "admin", "secret", None),
            admin("admin")
        );
    }

    #[test]
    fn rejected_when_password_wrong() {
        let header = format!("Basic {}", b64("admin:wrong"));
        assert_eq!(
            classify_default(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_username_wrong() {
        let header = format!("Basic {}", b64("evil:secret"));
        assert_eq!(
            classify_default(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_scheme_not_basic_and_no_sso() {
        let header = format!("Bearer {}", b64("admin:secret"));
        assert_eq!(
            classify_default(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_base64_invalid() {
        assert_eq!(
            classify_default(
                Some("Basic !!!not-base64!!!"),
                None,
                None,
                "admin",
                "secret",
                None
            ),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_decoded_has_no_colon() {
        let header = format!("Basic {}", b64("adminsecret"));
        assert_eq!(
            classify_default(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_decoded_is_invalid_utf8() {
        let raw = base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe, 0xfd]);
        let header = format!("Basic {}", raw);
        assert_eq!(
            classify_default(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn admin_when_password_contains_colon() {
        // Per RFC 7617 only the FIRST colon is the separator.
        let header = format!("Basic {}", b64("admin:p:a:s:s"));
        assert_eq!(
            classify_default(Some(&header), None, None, "admin", "p:a:s:s", None),
            admin("admin")
        );
    }

    #[test]
    fn role_and_identity_helpers() {
        let a = admin("admin");
        assert_eq!(a.role(), Role::Admin);
        assert_eq!(a.identity().unwrap().username, "admin");
        assert_eq!(AuthOutcome::Anonymous.role(), Role::Anonymous);
        assert_eq!(AuthOutcome::Anonymous.identity(), None);
        assert_eq!(AuthOutcome::Rejected.role(), Role::Anonymous);
        assert_eq!(AuthOutcome::Rejected.identity(), None);
        assert!(Role::Admin > Role::Sso);
        assert!(Role::Sso > Role::Anonymous);
    }

    #[test]
    fn sso_valid_bearer_yields_sso_role() {
        let token = mint(600, "alice");
        let header = format!("Bearer {}", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify_default(Some(&header), None, None, "admin", "secret", Some(&rt));
        match out {
            AuthOutcome::Sso(id) => {
                assert_eq!(id.username, "alice");
                assert_eq!(id.source, AuthSource::Sso);
            }
            other => panic!("expected Sso, got {other:?}"),
        }
    }

    #[test]
    fn sso_query_token_works() {
        let token = mint(600, "alice");
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify_default(None, None, Some(&token), "admin", "secret", Some(&rt));
        assert!(matches!(out, AuthOutcome::Sso(_)));
    }

    #[test]
    fn sso_cookie_works() {
        let token = mint(600, "alice");
        let cookie = format!("foo=bar; sso_access_token={}; baz=qux", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify_default(None, Some(&cookie), None, "admin", "secret", Some(&rt));
        assert!(matches!(out, AuthOutcome::Sso(_)));
    }

    #[test]
    fn basic_wins_over_valid_bearer_in_cookie() {
        // Authorization can carry only one scheme; this is the resolution
        // when Basic is in the header and a valid Bearer arrives via cookie.
        let token = mint(600, "alice");
        let basic = format!("Basic {}", b64("admin:secret"));
        let cookie = format!("sso_access_token={}", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify_default(
            Some(&basic),
            Some(&cookie),
            None,
            "admin",
            "secret",
            Some(&rt),
        );
        match out {
            AuthOutcome::Admin(id) => assert_eq!(id.source, AuthSource::Basic),
            other => panic!("expected Admin via Basic, got {other:?}"),
        }
    }

    #[test]
    fn broken_basic_does_not_block_valid_bearer_in_cookie() {
        // Authorization carries Basic with a wrong password; a valid Bearer
        // arrives via cookie. SSO must still pass.
        let token = mint(600, "alice");
        let basic = format!("Basic {}", b64("admin:wrong"));
        let cookie = format!("sso_access_token={}", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify_default(
            Some(&basic),
            Some(&cookie),
            None,
            "admin",
            "secret",
            Some(&rt),
        );
        assert!(matches!(out, AuthOutcome::Sso(_)));
    }

    #[test]
    fn expired_bearer_alone_yields_rejected() {
        // jsonwebtoken's default `Validation` carries 60s of leeway.
        // Mint with -600s so we are unambiguously outside the window.
        let token = mint(-600, "alice");
        let header = format!("Bearer {}", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify_default(Some(&header), None, None, "admin", "secret", Some(&rt));
        assert_eq!(out, AuthOutcome::Rejected);
    }

    #[test]
    fn allowlist_miss_yields_rejected() {
        let token = mint(600, "charlie");
        let header = format!("Bearer {}", token);
        let rt = sso_rt(AllowedUsers::List(
            ["alice".to_string()].into_iter().collect(),
        ));
        let out = classify_default(Some(&header), None, None, "admin", "secret", Some(&rt));
        assert_eq!(out, AuthOutcome::Rejected);
    }

    #[test]
    fn bearer_when_sso_disabled_is_rejected() {
        // Authorization: Bearer with no SsoRuntime configured: the Basic
        // branch can't parse it (not "Basic ..."), and SSO branch is None,
        // so a credential was attempted but nothing took it: Rejected.
        let token = mint(600, "alice");
        let header = format!("Bearer {}", token);
        let out = classify_default(Some(&header), None, None, "admin", "secret", None);
        assert_eq!(out, AuthOutcome::Rejected);
    }

    #[test]
    fn require_https_rejects_bearer_on_plain_http() {
        // sso_require_https = true and the transport is not secure → the
        // Bearer JWT is treated as a credential attempt that failed, so
        // the caller gets a 401 instead of an Anonymous fall-through.
        let token = mint(600, "alice");
        let header = format!("Bearer {}", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify(
            Some(&header),
            None,
            None,
            "admin",
            "secret",
            Some(&rt),
            https_only_transport(false),
        );
        assert_eq!(out, AuthOutcome::Rejected);
    }

    #[test]
    fn require_https_accepts_bearer_on_secure_hop() {
        let token = mint(600, "alice");
        let header = format!("Bearer {}", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify(
            Some(&header),
            None,
            None,
            "admin",
            "secret",
            Some(&rt),
            https_only_transport(true),
        );
        assert!(matches!(out, AuthOutcome::Sso(_)));
    }

    #[test]
    fn require_https_does_not_block_basic() {
        // Basic credentials live or die on the constant-time compare;
        // sso_require_https only gates the SSO branches.
        let header = format!("Basic {}", b64("admin:secret"));
        let out = classify(
            Some(&header),
            None,
            None,
            "admin",
            "secret",
            None,
            https_only_transport(false),
        );
        assert_eq!(out, admin("admin"));
    }

    #[test]
    fn require_https_leaves_anonymous_alone_when_no_sso_attempt() {
        // sso_require_https is on but the request carries no SSO source
        // at all — it stays Anonymous so the caller can still hit a
        // public read-only endpoint over plain HTTP.
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify(
            None,
            None,
            None,
            "admin",
            "secret",
            Some(&rt),
            https_only_transport(false),
        );
        assert_eq!(out, AuthOutcome::Anonymous);
    }

    #[test]
    fn require_https_off_keeps_plain_http_sso_working() {
        // The default policy (require_https = false) preserves
        // backward-compatible behaviour for the SSO-proxy-fronts-pg_doorman
        // deployment where the proxy → pg_doorman hop is private HTTP.
        let token = mint(600, "alice");
        let header = format!("Bearer {}", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify(
            Some(&header),
            None,
            None,
            "admin",
            "secret",
            Some(&rt),
            permissive_transport(),
        );
        assert!(matches!(out, AuthOutcome::Sso(_)));
    }
}
