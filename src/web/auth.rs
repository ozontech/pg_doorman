//! Basic-auth parser for the web mux.
//!
//! HTTP/1.1 `Authorization: Basic <base64(user:pass)>` header parsing
//! plus constant-time credential comparison.

use base64::Engine;
use subtle::ConstantTimeEq;

/// Authentication outcome for an inbound request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// Request carries no Authorization header.
    Anonymous,
    /// Authorization header present and matched the configured admin credentials.
    Admin,
    /// Authorization header present but malformed or did not match.
    Rejected,
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
        AuthOutcome::Admin
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

    #[test]
    fn anonymous_when_header_missing() {
        assert_eq!(classify(None, "admin", "secret"), AuthOutcome::Anonymous);
    }

    #[test]
    fn admin_when_credentials_match() {
        let header = format!("Basic {}", b64("admin:secret"));
        assert_eq!(
            classify(Some(&header), "admin", "secret"),
            AuthOutcome::Admin
        );
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
        assert_eq!(
            classify(Some(&header), "admin", "p:a:s:s"),
            AuthOutcome::Admin
        );
    }
}
