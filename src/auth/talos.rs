use crate::errors::Error;
use base64::prelude::*;
use jwt::{Header, PKeyWithDigest, RegisteredClaims, SignWithKey, Token, VerifyWithKey};
use once_cell::sync::Lazy;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Public};
use openssl::rsa::Rsa;
use serde_derive::{Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub async fn extract_talos_token(
    access_token: String,
    databases: Vec<String>,
) -> Result<TalosParsedToken, Error> {
    let key = get_key_from_token(&access_token)?;
    extract_talos_token_with_key(databases, key, access_token).await
}
pub static TALOS_KEYS: Lazy<RwLock<HashMap<String, PKeyWithDigest<Public>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

pub async fn load_talos_pub_key(key_filename: String) -> Result<(), Error> {
    let key = Path::new(&key_filename)
        .file_stem()
        .ok_or_else(|| Error::AuthError(format!("can't create filepath: {key_filename}")))?;

    let key = key.to_str().ok_or_else(|| {
        Error::AuthError(format!("can't convert filepath to string: {key_filename}"))
    })?;

    let pub_key_data =
        fs::read_to_string(&key_filename).map_err(|err| Error::JWTPubKey(err.to_string()))?;

    let pub_key = PKey::public_key_from_pem(pub_key_data.as_ref())
        .map_err(|err| Error::JWTPubKey(err.to_string()))?;
    let rs256_public_key = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: pub_key,
    };
    let mut guard_write = TALOS_KEYS.write().await;
    guard_write.insert(key.to_string(), rs256_public_key);
    Ok(())
}

/// Source used to choose a Talos pool user.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TalosUserSource {
    /// Pool user equals the JWT `clientId`.
    Personal,
    /// Pool user is `srv-<clientId>`.
    ServicePool,
    /// Pool user is the max token role.
    MaxRole,
}

/// Pool user selected for a Talos client.
#[derive(Debug, Clone)]
pub struct TalosResolution {
    pub username: String,
    pub source: TalosUserSource,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone)]
pub enum Role {
    ReadOnly = 1,
    ReadWrite = 2,
    Owner = 3,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RoleFromStr(());

impl FromStr for Role {
    type Err = RoleFromStr;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Role::Owner),
            "read_write" => Ok(Role::ReadWrite),
            "read_only" => Ok(Role::ReadOnly),
            _ => Err(RoleFromStr(())),
        }
    }
}

pub fn talos_role_to_string(r: Role) -> String {
    match r {
        Role::Owner => "owner".to_string(),
        Role::ReadWrite => "read_write".to_string(),
        Role::ReadOnly => "read_only".to_string(),
    }
}

/// Returns true when `client_id` can be used in pool lookup and logs.
fn is_routable_client_id(client_id: &str) -> bool {
    !client_id.is_empty() && client_id.bytes().all(|b| b >= 0x20 && b != 0x7F)
}

/// Selects a Talos pool user: `clientId`, `srv-<clientId>`, then max role.
///
/// Invalid `clientId` is ignored for routing, not for authentication.
pub fn resolve_talos_user(
    pool_name: &str,
    client_id: &str,
    max_role: Role,
    pool_exists: impl Fn(&str, &str) -> bool,
) -> TalosResolution {
    if is_routable_client_id(client_id) {
        if pool_exists(pool_name, client_id) {
            return TalosResolution {
                username: client_id.to_string(),
                source: TalosUserSource::Personal,
            };
        }
        let service_name = format!("srv-{client_id}");
        if pool_exists(pool_name, &service_name) {
            return TalosResolution {
                username: service_name,
                source: TalosUserSource::ServicePool,
            };
        }
    } else if !client_id.is_empty() {
        log::warn!(
            "[talos] client_id {client_id:?} contains control characters; using max-role pool"
        );
    }
    TalosResolution {
        username: talos_role_to_string(max_role),
        source: TalosUserSource::MaxRole,
    }
}

/// Logs the Talos routing choice without token material.
pub fn log_talos_routing(client_id: &str, pool_name: &str, role: Role, resolved: &TalosResolution) {
    let route = match resolved.source {
        TalosUserSource::Personal => "personal_pool",
        TalosUserSource::ServicePool => "service_pool",
        TalosUserSource::MaxRole => "max_role",
    };
    log::info!(
        "[talos] auth: client_id={} pool={} role={} username={} route={}",
        client_id,
        pool_name,
        talos_role_to_string(role),
        resolved.username,
        route,
    );
}

fn get_max_role(roles: Vec<String>) -> Result<Role, Error> {
    if roles.is_empty() {
        return Err(Error::AuthError("empty roles in talos token".to_string()));
    }

    roles
        .iter()
        .map(|role| {
            Role::from_str(role)
                .map_err(|_| Error::AuthError(format!("unsupported role: {role} in talos token")))
        })
        .collect::<Result<Vec<Role>, Error>>()?
        .into_iter()
        .max()
        .ok_or_else(|| Error::AuthError("can't find max role in talos token".to_string()))
}

#[derive(Serialize, Deserialize, Debug)]
struct TalosClaimsRoles {
    #[serde(rename = "roles")]
    roles: Vec<String>,
}
#[derive(Serialize, Deserialize, Debug)]
struct TalosClaims {
    #[serde(flatten)]
    default_claims: RegisteredClaims, // https://tools.ietf.org/html/rfc7519#page-9
    #[serde(rename = "clientId")]
    client_id: String,
    #[serde(rename = "resource_access")]
    resource_access: HashMap<String, TalosClaimsRoles>,
}

pub struct TalosParsedToken {
    pub role: Role,
    pub client_id: String,
    #[allow(dead_code)]
    pub valid_until: u64,
}

impl TalosClaims {
    fn validate(&self) -> Result<(), Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::JWTValidate(format!("Failed to get current time: {e}")))?
            .as_secs();

        // Check not_before claim
        if let Some(not_before) = self.default_claims.not_before {
            if now < not_before {
                return Err(Error::JWTValidate(format!(
                    "Token not yet valid. Current time: {now}, valid from: {not_before}"
                )));
            }
        }

        // Check expiration claim
        match self.default_claims.expiration {
            Some(expiration) => {
                if now > expiration {
                    return Err(Error::JWTValidate(format!(
                        "Token has expired. Current time: {now}, expired at: {expiration}"
                    )));
                }
            }
            None => {
                return Err(Error::JWTValidate(
                    "Token missing required expiration claim".to_string(),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct KidFromJSON {
    #[serde(rename = "kid")]
    kid: String,
}
/// Extracts the key identifier from the JWT header.
fn get_key_from_token(access_token: &str) -> Result<String, Error> {
    let header_part = access_token.split('.').next().ok_or_else(|| {
        Error::JWTValidate("JWT token must contain at least one dot separator".to_string())
    })?;

    // JWT использует URL-safe Base64 кодирование без padding
    // Преобразуем URL-safe символы в стандартные Base64 символы
    let base64_header = header_part.replace('-', "+").replace('_', "/");

    // Декодируем Base64 заголовок в байты
    let decoded_bytes = BASE64_STANDARD_NO_PAD
        .decode(&base64_header)
        .map_err(|err| {
            Error::JWTValidate(format!("Failed to decode JWT header as Base64: {err}"))
        })?;

    // Преобразуем байты в UTF-8 строку
    let header_json = String::from_utf8(decoded_bytes)
        .map_err(|err| Error::JWTValidate(format!("JWT header contains invalid UTF-8: {err}")))?;

    // Парсим JSON заголовок и извлекаем поле "kid"
    let kid_data: KidFromJSON = serde_json::from_str(&header_json)
        .map_err(|err| Error::JWTValidate(format!("Failed to parse JWT header JSON: {err}")))?;

    // Проверяем, что kid не пустой
    if kid_data.kid.is_empty() {
        return Err(Error::JWTValidate(
            "JWT header contains empty 'kid' field".to_string(),
        ));
    }

    Ok(kid_data.kid)
}

async fn extract_talos_token_with_key(
    databases: Vec<String>,
    key: String,
    access_token: String,
) -> Result<TalosParsedToken, Error> {
    let read_guard = TALOS_KEYS.read().await;

    let pub_key = read_guard.get(&key).ok_or_else(|| Error::JWTPubKey(format!(
            "Talos public key '{key}' not found in loaded keys. Make sure the key is loaded before token validation."
        ))
    )?;

    let token: Token<Header, TalosClaims, _> = VerifyWithKey::verify_with_key(access_token.as_str(), pub_key)
        .map_err(|err| Error::JWTValidate(format!(
                "Failed to verify JWT token signature with key '{key}': {err}. This could indicate an invalid token, wrong key, or token tampering."
            ))
        )?;

    let (_, claim) = token.into();
    claim.validate()?;

    let mut string_roles = vec![];
    for (k, v) in claim.resource_access {
        // k = postgres.stg:pgstats
        if let Some((_, resource_database)) = k.split_once(':') {
            if databases.iter().any(|db| resource_database == db) {
                string_roles.extend(v.roles);
                // No need to continue checking other databases once we've found a match
            }
        }
    }

    let max_role = get_max_role(string_roles)
        .map_err(|err| Error::AuthError(format!(
                "Failed to determine user role for databases {databases:?}: {err}. Token may not contain valid roles for the requested databases."
            ))
        )?;

    Ok(TalosParsedToken {
        role: max_role,
        client_id: claim.client_id,
        valid_until: claim.default_claims.expiration.unwrap(),
    })
}

#[allow(dead_code)]
async fn sign_with_jwt_priv_key(
    claims: TalosClaims,
    key_filename: String,
) -> Result<String, Error> {
    let priv_key_data =
        fs::read_to_string(&key_filename).map_err(|err| Error::JWTPrivKey(err.to_string()))?;

    let priv_key_rsa = Rsa::private_key_from_pem(priv_key_data.as_bytes())
        .map_err(|err| Error::JWTPrivKey(err.to_string()))?;

    let priv_key =
        PKey::from_rsa(priv_key_rsa).map_err(|err| Error::JWTPrivKey(err.to_string()))?;

    let rs256_priv_key = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: priv_key,
    };

    claims
        .sign_with_key(&rs256_priv_key)
        .map_err(|err| Error::JWTPrivKey(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Helper function to generate temporary RSA key pair
    fn generate_temp_rsa_keys() -> (NamedTempFile, NamedTempFile) {
        // Generate RSA key pair
        let rsa = Rsa::generate(2048).unwrap();

        // Create private key PEM
        let private_pem = rsa.private_key_to_pem().unwrap();
        let mut private_file = NamedTempFile::new().unwrap();
        private_file.write_all(&private_pem).unwrap();
        private_file.flush().unwrap();

        // Create public key PEM
        let public_pem = rsa.public_key_to_pem().unwrap();
        let mut public_file = NamedTempFile::new().unwrap();
        public_file.write_all(&public_pem).unwrap();
        public_file.flush().unwrap();

        (private_file, public_file)
    }

    #[tokio::test]
    async fn test_key() {
        let str = get_key_from_token(
            "eyJhbGciOiJSUzI1NiIsImtpZCI6IkJBb3JkTTktOXhIeERKZ1V5NUtMY2pCNWJMa3hpN1hNIiwidHlwIjoiSldUIn0.eyJhY3IiOjEs"
        ).unwrap();
        assert_eq!(str, "BAordM9-9xHxDJgUy5KLcjB5bLkxi7XM")
    }

    #[tokio::test]
    async fn test_key_invalid_format() {
        // Test with a token that doesn't contain dots
        let result = get_key_from_token("invalid_token_format");
        assert!(result.is_err());

        // Test with empty token
        let result = get_key_from_token("");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_max_role() {
        assert_eq!(
            get_max_role(vec![
                "owner".to_string(),
                "read_only".to_string(),
                "read_only".to_string()
            ])
            .unwrap(),
            Role::Owner
        )
    }

    #[tokio::test]
    async fn test_max_role_empty() {
        // Test with empty roles vector
        let result = get_max_role(vec![]);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_max_role_invalid() {
        // Test with invalid role
        let result = get_max_role(vec!["invalid_role".to_string()]);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_talos_role_to_string() {
        // Test all role conversions
        assert_eq!(talos_role_to_string(Role::Owner), "owner");
        assert_eq!(talos_role_to_string(Role::ReadWrite), "read_write");
        assert_eq!(talos_role_to_string(Role::ReadOnly), "read_only");
    }

    #[tokio::test]
    async fn test_claims_validate() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Valid claims (expiration in the future)
        let valid_claims = TalosClaims {
            default_claims: RegisteredClaims {
                expiration: Some(now + 3600), // 1 hour in the future
                not_before: Some(now - 3600), // 1 hour in the past
                ..Default::default()
            },
            client_id: "test-client".to_string(),
            resource_access: HashMap::new(),
        };
        assert!(valid_claims.validate().is_ok());

        // Invalid claims - expired token
        let expired_claims = TalosClaims {
            default_claims: RegisteredClaims {
                expiration: Some(now - 3600), // 1 hour in the past
                ..Default::default()
            },
            client_id: "test-client".to_string(),
            resource_access: HashMap::new(),
        };
        assert!(expired_claims.validate().is_err());

        // Invalid claims - token not yet valid
        let not_yet_valid_claims = TalosClaims {
            default_claims: RegisteredClaims {
                expiration: Some(now + 7200), // 2 hours in the future
                not_before: Some(now + 3600), // 1 hour in the future
                ..Default::default()
            },
            client_id: "test-client".to_string(),
            resource_access: HashMap::new(),
        };
        assert!(not_yet_valid_claims.validate().is_err());

        // Invalid claims - missing expiration
        let missing_expiration_claims = TalosClaims {
            default_claims: RegisteredClaims {
                expiration: None,
                ..Default::default()
            },
            client_id: "test-client".to_string(),
            resource_access: HashMap::new(),
        };
        assert!(missing_expiration_claims.validate().is_err());
    }

    #[tokio::test]
    async fn test_load_talos_pub_key() {
        let (_private_file, public_file) = generate_temp_rsa_keys();
        let public_path = public_file.path().to_str().unwrap().to_string();
        let key_name = public_file
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Clear any existing keys
        {
            let mut guard_write = TALOS_KEYS.write().await;
            guard_write.clear();
        }

        // Test loading a valid public key
        let result = load_talos_pub_key(public_path.clone()).await;
        assert!(result.is_ok());

        // Verify the key was loaded correctly
        {
            let guard_read = TALOS_KEYS.read().await;
            assert!(guard_read.contains_key(&key_name));
            assert_eq!(guard_read.len(), 1);
        }

        // Test loading a non-existent file
        let result = load_talos_pub_key("./non_existent_file.pem".to_string()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_generate_and_validate() {
        let (private_file, public_file) = generate_temp_rsa_keys();
        let private_path = private_file.path().to_str().unwrap().to_string();
        let public_path = public_file.path().to_str().unwrap().to_string();
        let key_name = public_file
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut claims = TalosClaims {
            default_claims: Default::default(),
            client_id: "client-id".to_string(),
            resource_access: HashMap::new(),
        };
        claims.resource_access.insert(
            "postgres.stg:database-1".to_string(),
            TalosClaimsRoles {
                roles: vec!["read_only".to_string()],
            },
        );
        claims.resource_access.insert(
            "postgres.stg:database".to_string(),
            TalosClaimsRoles {
                roles: vec!["owner".to_string()],
            },
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        claims.default_claims.expiration = Some(now + 2);
        let token = match sign_with_jwt_priv_key(claims, private_path).await {
            Ok(token) => token,
            Err(err) => panic!("{err:?}"),
        };
        load_talos_pub_key(public_path).await.unwrap();
        let result = extract_talos_token_with_key(
            vec!["database".to_string(), "database-1".to_string()],
            key_name,
            token,
        )
        .await
        .unwrap();
        assert_eq!(result.role, Role::Owner);
        assert_eq!(result.client_id, "client-id".to_string());
        assert_ne!(result.valid_until, 0);
    }

    #[tokio::test]
    async fn test_extract_talos_token() {
        // Instead of generating a token, we'll use a pre-formatted token with a known kid
        // This is the same token used in test_key which we know has a valid kid
        let token = "eyJhbGciOiJSUzI1NiIsImtpZCI6IkJBb3JkTTktOXhIeERKZ1V5NUtMY2pCNWJMa3hpN1hNIiwidHlwIjoiSldUIn0.eyJhY3IiOjEs";

        // Test with invalid token format
        let result = extract_talos_token(token.to_string(), vec!["db1".to_string()]).await;
        assert!(result.is_err(), "Expected error with incomplete token");

        // Test with completely invalid token
        let result =
            extract_talos_token("invalid_token".to_string(), vec!["db1".to_string()]).await;
        assert!(result.is_err(), "Expected error with invalid token");
    }

    #[tokio::test]
    async fn test_extract_talos_token_with_key_invalid() {
        // Test with invalid key
        let result = extract_talos_token_with_key(
            vec!["database".to_string()],
            "non_existent_key".to_string(),
            "valid_token_format".to_string(),
        )
        .await;
        assert!(result.is_err());
    }

    // --- resolve_talos_user + log_talos_routing tests ---

    #[test]
    fn talos_resolution_struct_is_constructible() {
        let resolved = TalosResolution {
            username: "owner".to_string(),
            source: TalosUserSource::MaxRole,
        };
        assert_eq!(resolved.source, TalosUserSource::MaxRole);
        assert_eq!(resolved.username, "owner");
    }

    #[test]
    fn resolve_personal_pool_wins() {
        let resolved = resolve_talos_user("billing_db", "billing-api", Role::Owner, |db, user| {
            db == "billing_db" && user == "billing-api"
        });
        assert_eq!(resolved.source, TalosUserSource::Personal);
        assert_eq!(resolved.username, "billing-api");
    }

    #[test]
    fn resolve_falls_through_to_service_pool() {
        let resolved =
            resolve_talos_user("billing_db", "billing-api", Role::ReadOnly, |_, user| {
                user == "srv-billing-api"
            });
        assert_eq!(resolved.source, TalosUserSource::ServicePool);
        assert_eq!(resolved.username, "srv-billing-api");
    }

    #[test]
    fn resolve_falls_through_to_max_role() {
        let resolved = resolve_talos_user("billing_db", "billing-api", Role::Owner, |_, _| false);
        assert_eq!(resolved.source, TalosUserSource::MaxRole);
        assert_eq!(resolved.username, "owner");
    }

    #[test]
    fn resolve_max_role_variations() {
        for (role, expected) in [
            (Role::Owner, "owner"),
            (Role::ReadWrite, "read_write"),
            (Role::ReadOnly, "read_only"),
        ] {
            let resolved = resolve_talos_user("db", "billing-api", role, |_, _| false);
            assert_eq!(resolved.source, TalosUserSource::MaxRole);
            assert_eq!(resolved.username, expected);
        }
    }

    #[test]
    fn resolve_skips_personal_when_client_id_empty() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let resolved = resolve_talos_user("billing_db", "", Role::Owner, |_, _| {
            calls.set(calls.get() + 1);
            true
        });
        assert_eq!(resolved.source, TalosUserSource::MaxRole);
        assert_eq!(
            calls.get(),
            0,
            "pool_exists must not be called for empty client_id"
        );
    }

    #[test]
    fn resolve_skips_personal_when_client_id_has_null() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let resolved = resolve_talos_user("billing_db", "foo\0bar", Role::Owner, |_, _| {
            calls.set(calls.get() + 1);
            true
        });
        assert_eq!(resolved.source, TalosUserSource::MaxRole);
        assert_eq!(
            calls.get(),
            0,
            "pool_exists must not be called for control-char client_id"
        );
    }

    #[test]
    fn resolve_skips_personal_when_client_id_has_control_char() {
        let resolved = resolve_talos_user("billing_db", "foo\u{0001}", Role::Owner, |_, _| true);
        assert_eq!(resolved.source, TalosUserSource::MaxRole);
    }

    #[test]
    fn log_talos_routing_does_not_panic_per_branch() {
        for source in [
            TalosUserSource::Personal,
            TalosUserSource::ServicePool,
            TalosUserSource::MaxRole,
        ] {
            let resolved = TalosResolution {
                username: "user".to_string(),
                source,
            };
            log_talos_routing("client", "db", Role::Owner, &resolved);
        }
    }
}
