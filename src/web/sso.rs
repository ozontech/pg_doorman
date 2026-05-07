//! SSO/JWT validation runtime. Built once on web-server start (and on
//! `RELOAD`) from `[web].sso_*` config. Holds the decoding key, the
//! `jsonwebtoken::Validation` config (RS256 + audience), and the
//! username allowlist. `validate` returns the username extracted from
//! the JWT on success, an error otherwise.

use std::collections::HashSet;
use std::path::Path;

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub enum AllowedUsers {
    /// Anyone whose JWT signature and audience check out is allowed.
    /// Equivalent to `sso_allowed_users = ["*"]` (or empty list).
    Any,
    /// A literal allowlist; only `preferred_username`/`sub` claims that
    /// match exactly may pass.
    List(HashSet<String>),
}

impl AllowedUsers {
    pub fn from_config(values: &[String]) -> Self {
        if values.is_empty() || values.iter().any(|v| v == "*") {
            AllowedUsers::Any
        } else {
            AllowedUsers::List(values.iter().cloned().collect())
        }
    }

    pub fn permits(&self, username: &str) -> bool {
        match self {
            AllowedUsers::Any => true,
            AllowedUsers::List(set) => set.contains(username),
        }
    }
}

#[derive(Deserialize)]
struct SsoClaims {
    preferred_username: Option<String>,
    sub: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SsoError {
    #[error("public key file not readable: {0}")]
    PublicKeyIo(#[from] std::io::Error),
    #[error("public key not valid PEM RSA: {0}")]
    PublicKeyDecode(jsonwebtoken::errors::Error),
    #[error("jwt validation failed: {0}")]
    Validation(jsonwebtoken::errors::Error),
    #[error("jwt has no preferred_username or sub claim")]
    NoUsername,
    #[error("user '{0}' not in SSO allowlist")]
    NotAllowed(String),
}

/// Holds everything `classify` needs to validate an inbound JWT plus the
/// proxy URL the SPA needs for the "Sign in via SSO" redirect.
pub struct SsoRuntime {
    decoding_key: DecodingKey,
    validation: Validation,
    allowed_users: AllowedUsers,
    proxy_url: Option<String>,
}

impl SsoRuntime {
    pub fn from_pem_file(
        public_key_path: &Path,
        audience: &[String],
        allowed_users: AllowedUsers,
        proxy_url: Option<String>,
    ) -> Result<Self, SsoError> {
        let pem = std::fs::read(public_key_path)?;
        Self::from_pem_bytes(&pem, audience, allowed_users, proxy_url)
    }

    pub fn from_pem_bytes(
        pem: &[u8],
        audience: &[String],
        allowed_users: AllowedUsers,
        proxy_url: Option<String>,
    ) -> Result<Self, SsoError> {
        let decoding_key = DecodingKey::from_rsa_pem(pem).map_err(SsoError::PublicKeyDecode)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;
        if !audience.is_empty() {
            validation.set_audience(audience);
        }
        Ok(SsoRuntime {
            decoding_key,
            validation,
            allowed_users,
            proxy_url,
        })
    }

    pub fn proxy_url(&self) -> Option<&str> {
        self.proxy_url.as_deref()
    }

    /// Validate a raw JWT string. Returns the resolved username on success.
    pub fn validate(&self, token: &str) -> Result<String, SsoError> {
        let data = decode::<SsoClaims>(token, &self.decoding_key, &self.validation)
            .map_err(SsoError::Validation)?;
        let username = data
            .claims
            .preferred_username
            .or(data.claims.sub)
            .ok_or(SsoError::NoUsername)?;
        if !self.allowed_users.permits(&username) {
            return Err(SsoError::NotAllowed(username));
        }
        Ok(username)
    }
}

#[cfg(test)]
pub(crate) mod test_keys {
    /// 2048-bit RSA keypair used **only** in unit tests. Never deploy this:
    /// the private key sits in version control by design, since regenerating
    /// it on every `cargo test` run would cost seconds and add openssl as a
    /// test-time runtime dependency.
    pub const PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDYXfQ1DLr3Unon
REbq7ul0/9++9mS6xBS8VspCfjfYWoHFsJQi5DuWIKhJdzcq2bB3UObV+cv2NFz0
Akfm0Vq2zZFv8AAsW8lHZNtgxYJR+8PnHL+TvdwigiJGPBfypGlLSEUhvcg/k1yo
pM8u0Sm246EyXM7wFN8j/Xrplrhtz8cD4gdb4gwqh5yL6xZvuzboKg/gip9TFNig
bOsReTA/HNMWDeUUkgGLgzvUJJ/PBv6ymVTcoylR42p8LnaZ/Qi/TlPzlcCvQ5NL
O1OzcFMRgKpvStdSnE3sB79Er1YDN7J9BerzqlvXHRu+jG314VVaJXqlnzzfkkdK
saR8uBbbAgMBAAECggEAIv7PSOVOOEJ2z3MTpVwPFoVsQw7HLA4a7Ht9K1QO5Ed/
ReJRk3Mm0BloHrnRinS7PhEvxNwqSSAfCLh1uLeT3I1TQK+o14PhAlMlyHkpouj9
vpu/wL2spUg3EvUVSoGdJjgCNnrjsKS9D+nYONJL1PDsWaD5N4uoq8GL58wg+GGe
tdW6W75lbUEV7HuFQ6ucSKIuJC+yWI6SkVHSeB/T0YtN8VLueF61j5nHM5Qk25MR
DbEQC7ji9daEF0n6TobMdgTHXGlxsCCJHuz1NC7J3bywB/Aw9HUg4Tt8kW5ya7WD
WsrHwLAWVnFLeDfg9FjvSMuqLBsoGM7JGPzj5I+NAQKBgQD0x2nPN3wqEgL7OI49
2I+RdtLWvA/Mnyl5CGZRlqatzKoJrnInzQNX2KkcQMQD262yVYCr3VMfElhyttF0
pPfO32+ZSiQLULmuStt+o5dRtbwDYTnWUXNWAdP5rOqiTzuavcymdW9sLH+ug08m
/hk/WlUGdAnKLWBZ+6NRs3RW6wKBgQDiSR02wRIyMFGL8DZbBm/ZgZt0fyV19QIY
sHUF0nuqBYAhZUTNCvVOWi+MzUJD2tAzztgu98FKnfxVh5Vive34EU0dK0Kcmcy2
1n6lT+Vq7wb6BcDNHNXCoRB3zfyjnWGMoVJStOltF/qzItB5GYwamC8NzJnKUQnj
EwRaF9Ej0QKBgQDS/RhdPuxNrxzWwpcJBCQsNInkAlJ0BDVRWEYpyXt+j815bt6D
JBnfnKeX7NOIp9B+yWiRu8KsI7oNlzvQGWpo7Pta3CdZgYmrjGbKL+R8z/Nxzlp2
O9r4pba7narZoQY1iahfSxZx3aFpIVIWwCuvCgQD/f16QcatiVPrVo3PZQKBgA92
K3bYTmP7hTbonO4vTGkyP+r/3RFoQlJpjDVvol+FrLGbd84C16wY4XLfe42jX+KK
WZ8r8psknF9DumNa0u3GUNyTXiPRJnm/wjuNcAGUf4eJ6RiaxchctQFao86SLF4t
j7BzCBgaMVkYIeKEalxO1bg9qKx4SRKo8/0r73BhAoGAbj+d1bSaTdxTvG0nsOPY
Pk2GSYbWa1Wk3YcV4DkB1wRVM6XvRYfS9UqyW7EeKK7csmLvEsfjLUdpsfuaIrHs
ZaTufgRVAyappMardriXaS+sdOIzW0tjMkMcovz1N6AMkRfCDq2DexTDkgQjHy9w
vktKmhUIVpWlVJcSxk2nz1w=
-----END PRIVATE KEY-----
";
    pub const PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA2F30NQy691J6J0RG6u7p
dP/fvvZkusQUvFbKQn432FqBxbCUIuQ7liCoSXc3Ktmwd1Dm1fnL9jRc9AJH5tFa
ts2Rb/AALFvJR2TbYMWCUfvD5xy/k73cIoIiRjwX8qRpS0hFIb3IP5NcqKTPLtEp
tuOhMlzO8BTfI/166Za4bc/HA+IHW+IMKoeci+sWb7s26CoP4IqfUxTYoGzrEXkw
PxzTFg3lFJIBi4M71CSfzwb+splU3KMpUeNqfC52mf0Iv05T85XAr0OTSztTs3BT
EYCqb0rXUpxN7Ae/RK9WAzeyfQXq86pb1x0bvoxt9eFVWiV6pZ8835JHSrGkfLgW
2wIDAQAB
-----END PUBLIC KEY-----
";
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Serialize)]
    struct Claims {
        exp: u64,
        aud: String,
        preferred_username: Option<String>,
        sub: Option<String>,
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn mint(claims: &Claims) -> String {
        let key = EncodingKey::from_rsa_pem(test_keys::PRIVATE_PEM.as_bytes()).unwrap();
        encode(&Header::new(Algorithm::RS256), claims, &key).unwrap()
    }

    fn runtime(allowed: AllowedUsers) -> SsoRuntime {
        SsoRuntime::from_pem_bytes(
            test_keys::PUBLIC_PEM.as_bytes(),
            &["pg_doorman".to_string()],
            allowed,
            Some("https://sso.example.com/oauth2/start".to_string()),
        )
        .unwrap()
    }

    #[test]
    fn happy_path_preferred_username() {
        let token = mint(&Claims {
            exp: now() + 600,
            aud: "pg_doorman".into(),
            preferred_username: Some("alice".into()),
            sub: Some("user-id-1".into()),
        });
        let rt = runtime(AllowedUsers::Any);
        assert_eq!(rt.validate(&token).unwrap(), "alice");
    }

    #[test]
    fn falls_back_to_sub_when_preferred_username_missing() {
        let token = mint(&Claims {
            exp: now() + 600,
            aud: "pg_doorman".into(),
            preferred_username: None,
            sub: Some("user-id-1".into()),
        });
        let rt = runtime(AllowedUsers::Any);
        assert_eq!(rt.validate(&token).unwrap(), "user-id-1");
    }

    #[test]
    fn rejects_expired_token() {
        let token = mint(&Claims {
            exp: now() - 600,
            aud: "pg_doorman".into(),
            preferred_username: Some("alice".into()),
            sub: None,
        });
        let rt = runtime(AllowedUsers::Any);
        assert!(matches!(
            rt.validate(&token).unwrap_err(),
            SsoError::Validation(_)
        ));
    }

    #[test]
    fn rejects_wrong_audience() {
        let token = mint(&Claims {
            exp: now() + 600,
            aud: "other-service".into(),
            preferred_username: Some("alice".into()),
            sub: None,
        });
        let rt = runtime(AllowedUsers::Any);
        assert!(matches!(
            rt.validate(&token).unwrap_err(),
            SsoError::Validation(_)
        ));
    }

    #[test]
    fn rejects_user_outside_allowlist() {
        let token = mint(&Claims {
            exp: now() + 600,
            aud: "pg_doorman".into(),
            preferred_username: Some("charlie".into()),
            sub: None,
        });
        let rt = runtime(AllowedUsers::List(
            ["alice".to_string(), "bob".to_string()]
                .into_iter()
                .collect(),
        ));
        assert!(matches!(
            rt.validate(&token).unwrap_err(),
            SsoError::NotAllowed(name) if name == "charlie"
        ));
    }

    #[test]
    fn star_allowlist_permits_any() {
        let token = mint(&Claims {
            exp: now() + 600,
            aud: "pg_doorman".into(),
            preferred_username: Some("charlie".into()),
            sub: None,
        });
        let rt = runtime(AllowedUsers::from_config(&["*".to_string()]));
        assert_eq!(rt.validate(&token).unwrap(), "charlie");
    }

    #[test]
    fn allowed_users_from_empty_is_any() {
        assert!(matches!(AllowedUsers::from_config(&[]), AllowedUsers::Any));
    }

    #[test]
    fn allowed_users_from_star_is_any() {
        assert!(matches!(
            AllowedUsers::from_config(&["*".into()]),
            AllowedUsers::Any
        ));
    }

    #[test]
    fn allowed_users_from_list_is_list() {
        let a = AllowedUsers::from_config(&["alice".into(), "bob".into()]);
        match a {
            AllowedUsers::List(set) => {
                assert!(set.contains("alice"));
                assert!(set.contains("bob"));
                assert!(!set.contains("charlie"));
            }
            _ => panic!("expected List"),
        }
    }
}
