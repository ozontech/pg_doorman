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
/// Extracts the key identifier (kid) from the JWT token header
///
/// A JWT token consists of three parts separated by dots: header.payload.signature
/// This function parses the header to get the kid (key ID), which is used
/// to select the correct public key for token verification
fn get_key_from_token(access_token: &str) -> Result<String, Error> {
    // JWT token has the format: header.payload.signature
    // We only need the header (first part before the dot)
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

        // For a more complete test, we would need to mock the extract_talos_token_with_key function
        // since we can't easily create a valid token with the correct structure in a test
        // This would require refactoring the code to make it more testable
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
}
