use bytes::{BufMut, BytesMut};
use log::error;
use tokio::io::AsyncReadExt;

use crate::auth::jwt::{new_claims, sign_with_jwt_priv_key};
use crate::auth::scram_client::ScramSha256;
use crate::config::User;
use crate::errors::{Error, ServerIdentifier};
use crate::messages::constants::*;
use crate::messages::{md5_hash_password, write_all_flush};

use super::stream::StreamInner;

/// Handles authentication during server startup.
/// Processes various authentication methods: SASL, MD5, clear password.
pub(crate) async fn handle_authentication(
    stream: &mut StreamInner,
    auth_code: i32,
    len: i32,
    user: &User,
    scram_client_auth: &mut Option<ScramSha256>,
    server_identifier: &ServerIdentifier,
) -> Result<(), Error> {
    match auth_code {
        AUTHENTICATION_SUCCESSFUL => Ok(()),

        // SASL authentication
        SASL => {
            let scram = scram_client_auth.as_mut().ok_or_else(|| {
                Error::ServerAuthError(
                    "server wants sasl auth, but it is not configured".into(),
                    server_identifier.clone(),
                )
            })?;

            let sasl_len = (len - 8) as usize;
            let mut sasl_auth = vec![0u8; sasl_len];
            stream.read_exact(&mut sasl_auth).await.map_err(|_| {
                Error::ServerStartupError(
                    "Failed to read SASL authentication message from server".into(),
                    server_identifier.clone(),
                )
            })?;

            let sasl_type = String::from_utf8_lossy(&sasl_auth[..sasl_len - 2]);
            if !sasl_type.contains(SCRAM_SHA_256) {
                error!("Unsupported SCRAM version: {sasl_type}");
                return Err(Error::ServerAuthError(
                    format!("Unsupported SCRAM version: {sasl_type}"),
                    server_identifier.clone(),
                ));
            }

            // Generate and send client message
            let sasl_response = scram.message();
            let mut res = BytesMut::new();
            res.put_u8(b'p');
            res.put_i32(4 + SCRAM_SHA_256.len() as i32 + 1 + 4 + sasl_response.len() as i32);
            res.put_slice(format!("{SCRAM_SHA_256}\0").as_bytes());
            res.put_i32(sasl_response.len() as i32);
            res.put(sasl_response);
            write_all_flush(stream, &res).await?;
            Ok(())
        }

        // SASL continuation
        SASL_CONTINUE => {
            let mut sasl_data = vec![0u8; (len - 8) as usize];
            stream.read_exact(&mut sasl_data).await.map_err(|_| {
                Error::ServerStartupError(
                    "Failed to read SASL continuation message from server".into(),
                    server_identifier.clone(),
                )
            })?;

            let msg = BytesMut::from(&sasl_data[..]);
            let sasl_response = scram_client_auth.as_mut().unwrap().update(&msg)?;

            let mut res = BytesMut::new();
            res.put_u8(b'p');
            res.put_i32(4 + sasl_response.len() as i32);
            res.put(sasl_response);
            write_all_flush(stream, &res).await?;
            Ok(())
        }

        // SASL final
        SASL_FINAL => {
            let mut sasl_final = vec![0u8; len as usize - 8];
            stream.read_exact(&mut sasl_final).await.map_err(|_| {
                Error::ServerStartupError("sasl final message".into(), server_identifier.clone())
            })?;

            scram_client_auth
                .as_mut()
                .unwrap()
                .finish(&BytesMut::from(&sasl_final[..]))?;
            Ok(())
        }

        // Clear password authentication
        AUTHENTICATION_CLEAR_PASSWORD => {
            if user.server_username.is_none() || user.server_password.is_none() {
                error!(
                    "authentication on server {}@{} with clear auth is not configured",
                    server_identifier.username, server_identifier.database,
                );
                return Err(Error::ServerAuthError(
                    "server wants clear password authentication, but auth for this server is not configured".into(),
                    server_identifier.clone(),
                ));
            }

            let server_password = user.server_password.as_ref().unwrap().clone();
            let server_username = user.server_username.as_ref().unwrap().clone();

            if !server_password.starts_with(JWT_PRIV_KEY_PASSWORD_PREFIX) {
                return Err(Error::ServerAuthError(
                    "plain password is not supported".into(),
                    server_identifier.clone(),
                ));
            }

            // Generate JWT token
            let claims = new_claims(server_username, std::time::Duration::from_secs(120));
            let token = sign_with_jwt_priv_key(
                claims,
                server_password
                    .strip_prefix(JWT_PRIV_KEY_PASSWORD_PREFIX)
                    .unwrap()
                    .to_string(),
            )
            .await
            .map_err(|err| Error::ServerAuthError(err.to_string(), server_identifier.clone()))?;

            let mut password_response = BytesMut::new();
            password_response.put_u8(b'p');
            password_response.put_i32(token.len() as i32 + 4 + 1);
            password_response.put_slice(token.as_bytes());
            password_response.put_u8(b'\0');
            stream.try_write(&password_response).map_err(|err| {
                Error::ServerAuthError(
                    format!("jwt authentication on the server failed: {err:?}"),
                    server_identifier.clone(),
                )
            })?;
            Ok(())
        }

        // MD5 password authentication
        MD5_ENCRYPTED_PASSWORD => {
            if user.server_username.is_none() || user.server_password.is_none() {
                error!(
                    "authentication for server {}@{} with md5 auth is not configured",
                    server_identifier.username, server_identifier.database,
                );
                return Err(Error::ServerAuthError(
                    "server wants md5 authentication, but auth for this server is not configured"
                        .into(),
                    server_identifier.clone(),
                ));
            }

            let server_username = user.server_username.as_ref().unwrap();
            let server_password = user.server_password.as_ref().unwrap();

            let mut salt = BytesMut::with_capacity(4);
            stream.read_buf(&mut salt).await.map_err(|err| {
                Error::ServerAuthError(
                    format!("md5 authentication on the server: {err:?}"),
                    server_identifier.clone(),
                )
            })?;

            let password_hash = md5_hash_password(
                server_username.as_str(),
                server_password.as_str(),
                salt.as_mut(),
            );

            let mut password_response = BytesMut::new();
            password_response.put_u8(b'p');
            password_response.put_i32(password_hash.len() as i32 + 4);
            password_response.put_slice(&password_hash);
            stream.try_write(&password_response).map_err(|err| {
                Error::ServerAuthError(
                    format!("md5 authentication on the server failed: {err:?}"),
                    server_identifier.clone(),
                )
            })?;
            Ok(())
        }

        _ => {
            error!(
                "this type of authentication on the server {}@{} is not supported, auth code: {}",
                server_identifier.username, server_identifier.database, auth_code
            );
            Err(Error::ServerAuthError(
                "authentication on the server is not supported".into(),
                server_identifier.clone(),
            ))
        }
    }
}
