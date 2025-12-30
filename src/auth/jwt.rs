// Standard library imports
use std::collections::HashMap;
use std::fs;
use std::ops::Add;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// External crate imports
use jwt::{Header, PKeyWithDigest, RegisteredClaims, SignWithKey, Token, VerifyWithKey};
use once_cell::sync::Lazy;
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Public};
use openssl::rsa::Rsa;
use serde_derive::{Deserialize, Serialize};
use tokio::sync::RwLock;

// Internal crate imports
use crate::errors::Error;

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

pub async fn sign_with_jwt_priv_key_data(
    claims: PreferredUsernameClaims,
    priv_key_data: String,
) -> Result<String, Error> {
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

pub async fn sign_with_jwt_priv_key(
    claims: PreferredUsernameClaims,
    key_filename: String,
) -> Result<String, Error> {
    let priv_key_data = match fs::read_to_string(key_filename.clone()) {
        Ok(data) => data,
        Err(err) => return Err(Error::JWTPrivKey(err.to_string())),
    };
    sign_with_jwt_priv_key_data(claims, priv_key_data).await
}

pub async fn load_jwt_pub_key_data(key_id: String, pub_key_data: String) -> Result<(), Error> {
    let pub_key = match PKey::public_key_from_pem(pub_key_data.as_ref()) {
        Ok(key) => key,
        Err(err) => return Err(Error::JWTPubKey(err.to_string())),
    };
    let rs256_public_key = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: pub_key,
    };
    let mut guard_write = KEYS.write().await;
    guard_write.insert(key_id, rs256_public_key);
    Ok(())
}

pub async fn load_jwt_pub_key(key_filename: String) -> Result<(), Error> {
    let pub_key_data = match fs::read_to_string(key_filename.clone()) {
        Ok(data) => data,
        Err(err) => return Err(Error::JWTPubKey(err.to_string())),
    };
    load_jwt_pub_key_data(key_filename, pub_key_data).await
}

pub async fn get_user_name_from_jwt(
    key_filename: String,
    input_token: String,
) -> Result<String, Error> {
    let read_guard = KEYS.read().await;
    let pub_key = match read_guard.get(&key_filename) {
        Some(key) => key,
        None => return Err(Error::JWTPubKey("key is not loaded".to_string())),
    };
    let token: Token<Header, PreferredUsernameClaims, _> =
        match VerifyWithKey::verify_with_key(input_token.as_str(), pub_key) {
            Ok(token) => token,
            Err(err) => return Err(Error::JWTValidate(err.to_string())),
        };
    let (_, claim) = token.into();
    claim.validate()?;
    Ok(claim.username)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jwt::{AlgorithmType, SignWithKey};

    fn generate_keys() -> (String, String) {
        let rsa = Rsa::generate(2048).unwrap();
        let priv_key = PKey::from_rsa(rsa.clone()).unwrap();
        let pub_key = PKey::from_rsa(rsa).unwrap();

        let priv_pem = priv_key.private_key_to_pem_pkcs8().unwrap();
        let pub_pem = pub_key.public_key_to_pem().unwrap();

        (
            String::from_utf8(priv_pem).unwrap(),
            String::from_utf8(pub_pem).unwrap(),
        )
    }

    #[tokio::test]
    async fn test_token() {
        let (priv_pem, pub_pem) = generate_keys();
        let key_id = "test_key".to_string();

        load_jwt_pub_key_data(key_id.clone(), pub_pem).await.unwrap();

        let rs256_private_key = PKeyWithDigest {
            digest: MessageDigest::sha256(),
            key: PKey::private_key_from_pem(priv_pem.as_ref()).unwrap(),
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
        get_user_name_from_jwt(key_id, token_str.to_string())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn test_generate_and_validate() {
        let (priv_pem, pub_pem) = generate_keys();
        let key_id = "test_key".to_string();

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

        let token = match sign_with_jwt_priv_key_data(claims, priv_pem).await {
            Ok(token) => token,
            Err(err) => panic!("{err:?}"),
        };

        load_jwt_pub_key_data(key_id.clone(), pub_pem).await.unwrap();

        let token_username = match get_user_name_from_jwt(key_id, token).await {
            Ok(username) => username,
            Err(err) => panic!("{err:?}"),
        };
        assert_eq!(username, token_username);
    }
}
