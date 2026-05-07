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

/// Inspect the value of an HTTP `Authorization` header (or `None` if absent),
/// compare against `admin_username`/`admin_password` in constant time, and
/// classify the outcome.
///
/// The comparison runs in constant time relative to the configured credentials
/// to deny timing oracles. We do **not** offer a way to learn whether the
/// username matched but the password didn't — both legs are checked together
/// without short-circuit.
pub fn classify(
    authorization_header: Option<&str>,
    admin_username: &str,
    admin_password: &str,
) -> AuthOutcome {
    let Some(header) = authorization_header else {
        return AuthOutcome::Anonymous;
    };
    let Some(b64) = header.strip_prefix("Basic ") else {
        return AuthOutcome::Rejected;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return AuthOutcome::Rejected;
    };
    let Ok(decoded_str) = std::str::from_utf8(&decoded) else {
        return AuthOutcome::Rejected;
    };
    let Some((user, pass)) = decoded_str.split_once(':') else {
        return AuthOutcome::Rejected;
    };
    // `&` instead of `&&`: avoids short-circuit, both legs always evaluated
    // so timing depends only on configured credential lengths.
    let matches = bool::from(user.as_bytes().ct_eq(admin_username.as_bytes()))
        & bool::from(pass.as_bytes().ct_eq(admin_password.as_bytes()));
    if matches {
        AuthOutcome::Admin(AuthIdentity {
            username: admin_username.to_string(),
            source: AuthSource::Basic,
        })
    } else {
        AuthOutcome::Rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    fn admin(name: &str) -> AuthOutcome {
        AuthOutcome::Admin(AuthIdentity {
            username: name.to_string(),
            source: AuthSource::Basic,
        })
    }

    #[test]
    fn anonymous_when_header_missing() {
        assert_eq!(classify(None, "admin", "secret"), AuthOutcome::Anonymous);
    }

    #[test]
    fn admin_when_credentials_match() {
        let header = format!("Basic {}", b64("admin:secret"));
        assert_eq!(classify(Some(&header), "admin", "secret"), admin("admin"));
    }

    #[test]
    fn rejected_when_password_wrong() {
        let header = format!("Basic {}", b64("admin:wrong"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_username_wrong() {
        let header = format!("Basic {}", b64("evil:secret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_scheme_not_basic() {
        let header = format!("Bearer {}", b64("admin:secret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_base64_invalid() {
        assert_eq!(
            classify(Some("Basic !!!not-base64!!!"), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_decoded_has_no_colon() {
        let header = format!("Basic {}", b64("adminsecret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn rejected_when_decoded_is_invalid_utf8() {
        let raw = base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe, 0xfd]);
        let header = format!("Basic {}", raw);
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn admin_when_password_contains_colon() {
        // Per RFC 7617 only the FIRST colon is the separator.
        let header = format!("Basic {}", b64("admin:p:a:s:s"));
        assert_eq!(classify(Some(&header), "admin", "p:a:s:s"), admin("admin"));
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
        // Role ordering: Admin > Sso > Anonymous.
        assert!(Role::Admin > Role::Sso);
        assert!(Role::Sso > Role::Anonymous);
    }
}
