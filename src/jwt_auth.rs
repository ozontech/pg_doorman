use crate::errors::{Error, JwtPrivKeyError, JwtPubKeyError, JwtValidateError};
use jwt::{Header, PKeyWithDigest, RegisteredClaims, SignWithKey, Token, VerifyWithKey};
use once_cell::sync::Lazy;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Public};
use openssl::rsa::Rsa;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::ops::Add;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

#[allow(dead_code)]
static KEYS: Lazy<RwLock<HashMap<String, PKeyWithDigest<Public>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

#[derive(Serialize, Deserialize)]
pub struct PreferredUsernameClaims {
    #[serde(flatten)]
    default_claims: RegisteredClaims, // https://tools.ietf.org/html/rfc7519#page-9
    #[serde(rename = "preferred_username")]
    username: String, // additional
}

pub fn new_claims(username: String, duration: Duration) -> PreferredUsernameClaims {
    let mut result = PreferredUsernameClaims {
        default_claims: RegisteredClaims::default(),
        username,
    };
    let time = SystemTime::now()
        .add(duration)
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    result.default_claims.expiration = Some(time);
    result
}

impl PreferredUsernameClaims {
    fn validate(&self) -> Result<(), JwtValidateError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if let Some(val) = self.default_claims.not_before {
            if now < val {
                return Err(JwtValidateError::NotBefore);
            }
        }

        let Some(expiration) = self.default_claims.expiration else {
            return Err(JwtValidateError::NoExpiration);
        };
        if now > expiration {
            return Err(JwtValidateError::Expiration);
        }

        Ok(())
    }
}

pub async fn sign_with_jwt_priv_key(
    claims: PreferredUsernameClaims,
    key_filename: &str,
) -> Result<String, JwtPrivKeyError> {
    let priv_key_data = fs::read_to_string(key_filename)?;
    let priv_key_rsa = Rsa::private_key_from_pem(priv_key_data.as_bytes())?;
    let priv_key = PKey::from_rsa(priv_key_rsa)?;
    let rs256_priv_key = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: priv_key,
    };

    Ok(claims.sign_with_key(&rs256_priv_key)?)
}

pub async fn load_jwt_pub_key(key_filename: String) -> Result<(), JwtPubKeyError> {
    let pub_key_data = fs::read_to_string(key_filename.clone())?;
    let pub_key = PKey::public_key_from_pem(pub_key_data.as_ref())?;
    let rs256_public_key = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: pub_key,
    };
    let mut guard_write = KEYS.write().await;
    guard_write.insert(key_filename, rs256_public_key);
    Ok(())
}

pub async fn get_user_name_from_jwt(
    key_filename: String,
    input_token: String,
) -> Result<String, Error> {
    let read_guard = KEYS.read().await;
    let pub_key = read_guard
        .get(&key_filename)
        .ok_or(JwtPubKeyError::KeyNotLoaded)?;

    let token: Token<Header, PreferredUsernameClaims, _> =
        VerifyWithKey::verify_with_key(input_token.as_str(), pub_key)
            .map_err(JwtValidateError::from)?;
    let (_, claim) = token.into();
    claim.validate()?;
    Ok(claim.username)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jwt::{AlgorithmType, SignWithKey};

    #[tokio::test]
    async fn test_token() {
        load_jwt_pub_key("./tests/data/jwt/public.pem".to_string())
            .await
            .unwrap();
        let private_pem = fs::read_to_string("./tests/data/jwt/private.pem").unwrap();
        let rs256_private_key = PKeyWithDigest {
            digest: MessageDigest::sha256(),
            key: PKey::private_key_from_pem(private_pem.as_ref()).unwrap(),
        };
        let header = Header {
            algorithm: AlgorithmType::Rs256,
            ..Default::default()
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut claims = PreferredUsernameClaims {
            default_claims: Default::default(),
            username: "test".to_string(),
        };
        claims.default_claims.expiration = Some(now + 2);
        let signed_token = Token::new(header, claims)
            .sign_with_key(&rs256_private_key)
            .unwrap();
        let token_str = signed_token.as_str();
        get_user_name_from_jwt(
            "./tests/data/jwt/public.pem".to_string(),
            token_str.to_string(),
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn test_generate_and_validate() {
        let username = "test";
        let mut claims = PreferredUsernameClaims {
            default_claims: Default::default(),
            username: username.to_string(),
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        claims.default_claims.expiration = Some(now + 2);
        let token = match sign_with_jwt_priv_key(claims, "./tests/data/jwt/private.pem").await {
            Ok(token) => token,
            Err(err) => panic!("{:?}", err),
        };
        load_jwt_pub_key("./tests/data/jwt/public.pem".to_string())
            .await
            .unwrap();
        let token_username =
            match get_user_name_from_jwt("./tests/data/jwt/public.pem".to_string(), token).await {
                Ok(username) => username,
                Err(err) => panic!("{:?}", err),
            };
        assert_eq!(username, token_username);
    }
}
