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

/// Classify an inbound HTTP request into an `AuthOutcome`. Recognises:
///
/// - `Authorization: Basic <b64(user:pass)>` against the admin
///   credentials (constant-time compare; success → `Admin`).
/// - `Authorization: Bearer <jwt>`, `?token=<jwt>` and
///   `Cookie: sso_access_token=<jwt>` validated against `sso` (success →
///   `Sso`).
///
/// Order of preference: Basic > Bearer header > query > cookie. Basic
/// always wins outright — a known admin password is the strongest
/// credential the caller can present. A *broken* Basic does not block a
/// valid SSO token: the function then tries the SSO sources in order.
///
/// The Basic comparison runs in constant time relative to the configured
/// credentials to deny timing oracles. Both username and password legs
/// are checked together without short-circuit (see the `&` operator
/// inside the implementation).
pub fn classify(
    authorization_header: Option<&str>,
    cookie_header: Option<&str>,
    query_token: Option<&str>,
    admin_username: &str,
    admin_password: &str,
    sso: Option<&crate::web::sso::SsoRuntime>,
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
    if let Some(rt) = sso {
        if let Some(token) = bearer_token {
            if let Ok(username) = rt.validate(token) {
                return AuthOutcome::Sso(AuthIdentity {
                    username,
                    source: AuthSource::Sso,
                });
            }
        }
        if let Some(token) = query_token {
            tried = true;
            if let Ok(username) = rt.validate(token) {
                return AuthOutcome::Sso(AuthIdentity {
                    username,
                    source: AuthSource::Sso,
                });
            }
        }
        if let Some(token) = cookie_header.and_then(find_sso_cookie) {
            tried = true;
            if let Ok(username) = rt.validate(token) {
                return AuthOutcome::Sso(AuthIdentity {
                    username,
                    source: AuthSource::Sso,
                });
            }
        }
    }

    if tried {
        AuthOutcome::Rejected
    } else {
        AuthOutcome::Anonymous
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
    use crate::web::sso::{test_keys, AllowedUsers, SsoRuntime};
    use jsonwebtoken::{encode, Algorithm as JwtAlg, EncodingKey, Header};
    use serde::Serialize;
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

    #[derive(Serialize)]
    struct TestClaims {
        exp: u64,
        aud: String,
        preferred_username: Option<String>,
        sub: Option<String>,
    }

    fn mint(exp_offset: i64, name: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = TestClaims {
            exp: (now + exp_offset) as u64,
            aud: "pg_doorman".into(),
            preferred_username: Some(name.into()),
            sub: None,
        };
        let key = EncodingKey::from_rsa_pem(test_keys::PRIVATE_PEM.as_bytes()).unwrap();
        encode(&Header::new(JwtAlg::RS256), &claims, &key).unwrap()
    }

    fn sso_rt(allowed: AllowedUsers) -> SsoRuntime {
        SsoRuntime::from_pem_bytes(
            test_keys::PUBLIC_PEM.as_bytes(),
            &["pg_doorman".to_string()],
            allowed,
            None,
        )
        .unwrap()
    }

    #[test]
    fn anonymous_when_header_missing() {
        assert_eq!(
            classify(None, None, None, "admin", "secret", None),
            AuthOutcome::Anonymous
        );
    }

    #[test]
    fn admin_when_credentials_match() {
        let header = format!("Basic {}", b64("admin:secret"));
        assert_eq!(
            classify(Some(&header), None, None, "admin", "secret", None),
            admin("admin")
        );
    }

    #[test]
    fn rejected_when_password_wrong() {
        let header = format!("Basic {}", b64("admin:wrong"));
        assert_eq!(
            classify(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_username_wrong() {
        let header = format!("Basic {}", b64("evil:secret"));
        assert_eq!(
            classify(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_scheme_not_basic_and_no_sso() {
        let header = format!("Bearer {}", b64("admin:secret"));
        assert_eq!(
            classify(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_base64_invalid() {
        assert_eq!(
            classify(
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
            classify(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_decoded_is_invalid_utf8() {
        let raw = base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe, 0xfd]);
        let header = format!("Basic {}", raw);
        assert_eq!(
            classify(Some(&header), None, None, "admin", "secret", None),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn admin_when_password_contains_colon() {
        // Per RFC 7617 only the FIRST colon is the separator.
        let header = format!("Basic {}", b64("admin:p:a:s:s"));
        assert_eq!(
            classify(Some(&header), None, None, "admin", "p:a:s:s", None),
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
        let out = classify(Some(&header), None, None, "admin", "secret", Some(&rt));
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
        let out = classify(None, None, Some(&token), "admin", "secret", Some(&rt));
        assert!(matches!(out, AuthOutcome::Sso(_)));
    }

    #[test]
    fn sso_cookie_works() {
        let token = mint(600, "alice");
        let cookie = format!("foo=bar; sso_access_token={}; baz=qux", token);
        let rt = sso_rt(AllowedUsers::Any);
        let out = classify(None, Some(&cookie), None, "admin", "secret", Some(&rt));
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
        let out = classify(
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
        let out = classify(
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
        let out = classify(Some(&header), None, None, "admin", "secret", Some(&rt));
        assert_eq!(out, AuthOutcome::Rejected);
    }

    #[test]
    fn allowlist_miss_yields_rejected() {
        let token = mint(600, "charlie");
        let header = format!("Bearer {}", token);
        let rt = sso_rt(AllowedUsers::List(
            ["alice".to_string()].into_iter().collect(),
        ));
        let out = classify(Some(&header), None, None, "admin", "secret", Some(&rt));
        assert_eq!(out, AuthOutcome::Rejected);
    }

    #[test]
    fn bearer_when_sso_disabled_is_rejected() {
        // Authorization: Bearer with no SsoRuntime configured: the Basic
        // branch can't parse it (not "Basic ..."), and SSO branch is None,
        // so a credential was attempted but nothing took it: Rejected.
        let token = mint(600, "alice");
        let header = format!("Bearer {}", token);
        let out = classify(Some(&header), None, None, "admin", "secret", None);
        assert_eq!(out, AuthOutcome::Rejected);
    }
}
