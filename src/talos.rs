use crate::errors::Error;
use once_cell::sync::Lazy;
use base64::prelude::*;
use jwt::{Header, PKeyWithDigest, RegisteredClaims, SignWithKey, Token, VerifyWithKey};
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Public};
use openssl::rsa::Rsa;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub async fn extract_talos_token(access_token: String, databases: Vec<String>) -> Result<TalosParsedToken, Error> {
    let key = get_key_from_token(access_token.clone())?;
    extract_talos_token_with_key(databases, key, access_token.clone()).await
}
pub static TALOS_KEYS: Lazy<RwLock<HashMap<String, PKeyWithDigest<Public>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

pub async fn load_talos_pub_key(key_filename: String) -> Result<(), Error> {
    let key = match Path::new(key_filename.as_str()).file_stem() {
        Some(key) => key,
        None => return Err(Error::AuthError(format!("can't create filepath: {}", key_filename)))
    };
    let key = match key.to_str() {
        Some(k) => k,
        None => return Err(Error::AuthError(format!("can't convert filepath to string: {}", key_filename)))
    };
    let pub_key_data = match fs::read_to_string(key_filename.clone()) {
        Ok(data) => data,
        Err(err) => return Err(Error::JWTPubKey(err.to_string())),
    };
    let pub_key = match PKey::public_key_from_pem(pub_key_data.as_ref()) {
        Ok(key) => key,
        Err(err) => return Err(Error::JWTPubKey(err.to_string())),
    };
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
        return if s == "owner" {
            Ok(Role::Owner)
        } else if s == "read_write" {
            Ok(Role::ReadWrite)
        } else if s == "read_only" {
            Ok(Role::ReadOnly)
        } else {
            Err(RoleFromStr(()))
        };
    }
}

fn max_role(roles: Vec<String>) -> Result<Role, Error> {
    if roles.is_empty() {
        return Err(Error::AuthError("empty roles in talos token".to_string()));
    }
    let mut talos_roles: Vec<Role> = vec![];
    for k in roles {
        talos_roles.push(match Role::from_str(k.as_str()) {
            Ok(r) => r,
            Err(_) => {
                return Err(Error::AuthError(
                    format!("unsupported role: {} in talos token", k).to_string(),
                ))
            }
        })
    }
    match talos_roles.iter().max() {
        Some(max) => Ok((*max).into()),
        None => Err(Error::AuthError(
            "can't find max role in talos token".to_string(),
        )),
    }
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
    role: Role,
    client_id: String,
    valid_until: u64,
}

impl TalosClaims {
    fn validate(&self) -> Result<(), Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if let Some(val) = self.default_claims.not_before {
            if now < val {
                return Err(Error::JWTValidate("not before".to_string()));
            }
        }
        if let Some(val) = self.default_claims.expiration {
            if now > val {
                return Err(Error::JWTValidate("expiration".to_string()));
            }
        } else {
            return Err(Error::JWTValidate("empty expiration".to_string()));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct KidFromJSON {
    #[serde(rename = "kid")]
    kid: String,
}
pub fn get_key_from_token(access_token: String) -> Result<String, Error> {
    let mut parts = access_token.split(".");
    let base64_json_part = match parts.next() {
        Some(str) => str,
        None => {
            return Err(Error::AuthError(
                "can't find first `.` in token".to_string(),
            ))
        }
    };
    let base64_json_part = base64_json_part.replace("-", "+").replace("_", "/");
    let base64_json_part = match BASE64_STANDARD_NO_PAD.decode(&base64_json_part) {
        Ok(a) => match String::from_utf8(a) {
            Ok(str) => str,
            Err(err) => {
                return Err(Error::AuthError(format!(
                    "decode string from token: {}",
                    err
                )))
            }
        },
        Err(err) => {
            return Err(Error::AuthError(format!(
                "base64 decode `{}`: {}",
                base64_json_part, err
            )))
        }
    };
    match serde_json::from_str::<KidFromJSON>(base64_json_part.as_str()) {
        Ok(s) => Ok(s.kid),
        Err(err) => Err(Error::AuthError(err.to_string())),
    }
}

async fn extract_talos_token_with_key(
    databases: Vec<String>,
    key: String,
    access_token: String,
) -> Result<TalosParsedToken, Error> {
    let pub_key;
    let read_guard = TALOS_KEYS.read().await;
    pub_key = match read_guard.get(&key) {
        Some(key) => key,
        None => return Err(Error::JWTPubKey(format!("talos key `{}` is not loaded", key))),
    };
    let token: Token<Header, TalosClaims, _> =
        match VerifyWithKey::verify_with_key(access_token.as_str(), pub_key) {
            Ok(token) => token,
            Err(err) => return Err(Error::JWTValidate(err.to_string())),
        };
    let (_, claim) = token.into();
    claim.validate()?;
    let mut string_roles = vec![];
    for (k, v) in claim.resource_access {
        // k = postgres.stg:pgstats
        let resource: Vec<&str> = k.split(":").collect();
        if resource.len() != 2 {
            continue;
        }
        let resource_database = resource[1];
        for database in databases.iter() {
            if resource_database == database {
                string_roles.append(v.roles.clone().as_mut());
                break;
            }
        }
    }
    let max_role = match max_role(string_roles) {
        Ok(r) => r,
        Err(err) => return Err(err),
    };
    Ok(TalosParsedToken {
        role: max_role,
        client_id: claim.client_id,
        valid_until: claim.default_claims.expiration.unwrap(),
    })
}

async fn sign_with_jwt_priv_key(
    claims: TalosClaims,
    key_filename: String,
) -> Result<String, Error> {
    let priv_key_data = match fs::read_to_string(key_filename.clone()) {
        Ok(data) => data,
        Err(err) => return Err(Error::JWTPrivKey(err.to_string())),
    };
    let priv_key_rsa = match Rsa::private_key_from_pem(priv_key_data.as_bytes()) {
        Ok(rsa) => rsa,
        Err(err) => return Err(Error::JWTPrivKey(err.to_string())),
    };
    let priv_key = match PKey::from_rsa(priv_key_rsa) {
        Ok(data) => data,
        Err(err) => return Err(Error::JWTPrivKey(err.to_string())),
    };
    let rs256_priv_key = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: priv_key,
    };
    let data = match claims.sign_with_key(&rs256_priv_key) {
        Ok(data) => data,
        Err(err) => return Err(Error::JWTPrivKey(err.to_string())),
    };
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_key() {
        let str = get_key_from_token(
            "eyJhbGciOiJSUzI1NiIsImtpZCI6IkJBb3JkTTktOXhIeERKZ1V5NUtMY2pCNWJMa3hpN1hNIiwidHlwIjoiSldUIn0.eyJhY3IiOjEs".to_string()
        ).unwrap();
        assert_eq!(str, "BAordM9-9xHxDJgUy5KLcjB5bLkxi7XM")
    }

    #[tokio::test]
    async fn test_max_role() {
        assert_eq!(
            max_role(vec![
                "owner".to_string(),
                "read_only".to_string(),
                "read_only".to_string()
            ])
            .unwrap(),
            Role::Owner
        )
    }

    #[tokio::test]
    async fn test_generate_and_validate() {
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
        let token = match sign_with_jwt_priv_key(claims, "./tests/data/jwt/private.pem".to_string())
            .await
        {
            Ok(token) => token,
            Err(err) => panic!("{:?}", err),
        };
        load_talos_pub_key("./tests/data/jwt/public.pem".to_string())
            .await
            .unwrap();
        let result = extract_talos_token_with_key(
            vec!["database".to_string(), "database-1".to_string()],
            "public".to_string(),
            token,
        )
        .await
        .unwrap();
        assert_eq!(result.role, Role::Owner);
        assert_eq!(result.client_id, "client-id".to_string());
        assert_ne!(result.valid_until, 0);
    }
}
