pub mod hba;
#[cfg(test)]
mod hba_eval_tests;
pub mod jwt;
pub mod pam;
pub mod scram;
pub mod scram_client;
pub mod talos;

// Standard library imports
use std::marker::Unpin;

// External crate imports
use crate::auth::hba::CheckResult;
use log::{error, info, warn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
// Internal crate imports
use crate::auth::jwt::get_user_name_from_jwt;
use crate::auth::pam::pam_auth;
use crate::auth::scram::{
    parse_client_final_message, parse_client_first_message, parse_server_secret,
    prepare_server_final_message, prepare_server_first_response,
};
use crate::config::{get_config, PoolMode};
use crate::errors::{ClientIdentifier, Error};
use crate::messages::constants::{
    JWT_PUB_KEY_PASSWORD_PREFIX, MD5_PASSWORD_PREFIX, SASL_CONTINUE, SASL_FINAL, SCRAM_SHA_256,
};
use crate::messages::{
    error_response, error_response_terminal, md5_challenge, md5_hash_password,
    md5_hash_second_pass, plain_password_challenge, read_password, scram_server_response,
    scram_start_challenge, vec_to_string, wrong_password,
};
use crate::pool::{get_pool, ConnectionPool};
use crate::server::ServerParameters;

/// Authenticate a user based on the provided parameters
pub async fn authenticate<S, T>(
    read: &mut S,
    write: &mut T,
    admin: bool,
    client_identifier: &ClientIdentifier,
    pool_name: &str,
    username_from_parameters: &str,
) -> Result<(bool, ServerParameters, bool), Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    let mut prepared_statements_enabled = false;

    // Authenticate admin user.
    let (transaction_mode, server_parameters) = if admin {
        if client_identifier.hba_md5 == CheckResult::Trust
            || client_identifier.hba_scram == CheckResult::Trust
        {
            info!(
                "HBA trust for admin user: {username_from_parameters} from: {:?}.",
                client_identifier.addr
            );
            return Ok((false, ServerParameters::admin(), false));
        }
        if client_identifier.hba_md5 == CheckResult::Deny
            || client_identifier.hba_scram == CheckResult::Deny
        {
            let error = Error::AuthError(format!(
                "HBA failed for admin user: {username_from_parameters}"
            ));
            warn!("{error}");
            wrong_password(write, username_from_parameters).await?;
            return Err(error);
        }
        authenticate_admin(read, write, username_from_parameters).await?
    }
    // Authenticate normal user.
    else {
        authenticate_normal_user(
            read,
            write,
            client_identifier,
            pool_name,
            username_from_parameters,
            &mut prepared_statements_enabled,
        )
        .await?
    };

    Ok((
        transaction_mode,
        server_parameters,
        prepared_statements_enabled,
    ))
}

/// Authenticate an admin user with MD5
async fn authenticate_admin<S, T>(
    read: &mut S,
    write: &mut T,
    username_from_parameters: &str,
) -> Result<(bool, ServerParameters), Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    // Authenticate admin user with md5.
    let salt = md5_challenge(write).await?;
    let password_response = read_password(read).await?;
    let config = get_config();

    // Compare server and client hashes.
    let password_hash = md5_hash_password(
        &config.general.admin_username,
        &config.general.admin_password,
        &salt,
    );

    if password_hash != password_response {
        let error = Error::AuthError(format!(
            "Invalid password for admin user: {username_from_parameters}"
        ));

        warn!("{error}");
        wrong_password(write, username_from_parameters).await?;

        return Err(error);
    }

    Ok((false, ServerParameters::admin()))
}

/// Authenticate a normal user with various methods
fn eval_hba_for_pool_password(pool_password: &str, ci: &ClientIdentifier) -> CheckResult {
    // Determine HBA outcome based on stored pool password type and HBA checks attached to client identifier
    if ci.is_talos {
        // Already authenticated upstream, allow normal auth flow (not a Trust, but no HBA block)
        return CheckResult::Allow;
    }

    // Empty password is allowed only when HBA is trust for either method
    if pool_password.is_empty()
        && (ci.hba_md5 == CheckResult::Trust || ci.hba_scram == CheckResult::Trust)
    {
        return CheckResult::Trust;
    }

    if pool_password.starts_with(SCRAM_SHA_256) {
        // If SCRAM is trusted or MD5 trust is allowed while SCRAM is not matched, treat as trust
        if ci.hba_scram == CheckResult::Trust
            || (ci.hba_scram == CheckResult::NotMatched && ci.hba_md5 == CheckResult::Trust)
        {
            return CheckResult::Trust;
        }

        // Explicit rejections or no matching rules result in deny
        if ci.hba_scram == CheckResult::Deny
            || (ci.hba_scram == CheckResult::NotMatched
                && (ci.hba_md5 == CheckResult::Deny || ci.hba_md5 == CheckResult::NotMatched))
        {
            return CheckResult::Deny;
        }

        // Otherwise, a password exchange is allowed
        return CheckResult::Allow;
    }

    if pool_password.starts_with(MD5_PASSWORD_PREFIX) {
        if ci.hba_md5 == CheckResult::Trust {
            return CheckResult::Trust;
        }
        if ci.hba_md5 == CheckResult::NotMatched || ci.hba_md5 == CheckResult::Deny {
            return CheckResult::Deny;
        }
        return CheckResult::Allow;
    }

    // For other auth kinds (JWT/PAM/unknown), the HBA rules here are not applicable.
    CheckResult::Allow
}

async fn authenticate_normal_user<S, T>(
    read: &mut S,
    write: &mut T,
    client_identifier: &ClientIdentifier,
    pool_name: &str,
    username_from_parameters: &str,
    prepared_statements_enabled: &mut bool,
) -> Result<(bool, ServerParameters), Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    let mut pool = match get_pool(pool_name, client_identifier.username.as_str()) {
        Some(pool) => pool,
        None => {
            error_response(
                write,
                &format!(
                    "No connection pool configured for database: {pool_name}, user: {username_from_parameters}. Please check your connection parameters and ensure the database/username is properly configured."
                ),
                "3D000",
            )
            .await?;

            return Err(Error::AuthError(format!("No connection pool configured for database: {pool_name}, user: {username_from_parameters}")));
        }
    };

    let pool_password = pool.settings.user.password.clone();

    // Evaluate HBA once for this connection
    let hba_decision = eval_hba_for_pool_password(&pool_password, client_identifier);
    if hba_decision == CheckResult::Deny {
        error_response_terminal(
        write,
        format!(
            "Connection with scram password from IP address {} to {}@{} is not permitted by HBA configuration. Please contact your database administrator.",
            client_identifier.addr, username_from_parameters, pool_name
        )
            .as_str(),
        "28000",
    )
        .await?;
        return Err(Error::HbaForbiddenError(format!(
        "Connection with scram not permitted by HBA configuration for client: {} from address: {:?}",
        client_identifier, client_identifier.addr,
    )));
    }

    if client_identifier.is_talos || hba_decision == CheckResult::Trust {
        // Pass, client already authenticated (talos) or HBA Trust
    } else if pool.settings.user.auth_pam_service.is_some() {
        authenticate_with_pam(read, write, &pool, username_from_parameters).await?;
    } else if pool_password.starts_with(SCRAM_SHA_256) {
        authenticate_with_scram(
            read,
            write,
            pool_password.as_str(),
            username_from_parameters,
        )
        .await?;
    } else if pool_password.starts_with(MD5_PASSWORD_PREFIX) {
        authenticate_with_md5(
            read,
            write,
            pool_password.as_str(),
            username_from_parameters,
            &pool,
        )
        .await?;
    } else if pool_password.starts_with(JWT_PUB_KEY_PASSWORD_PREFIX) {
        authenticate_with_jwt(
            read,
            write,
            pool_password
                .strip_prefix(JWT_PUB_KEY_PASSWORD_PREFIX)
                .unwrap()
                .to_string(),
            username_from_parameters,
        )
        .await?;
    } else {
        warn!("Unsupported password type for user {username_from_parameters}: {pool_password}");
        error_response_terminal(
            write,
            "Authentication method not supported. Please contact your database administrator.",
            "28P01",
        )
        .await?;
        return Err(Error::AuthError(format!(
            "Unsupported authentication method for user: {username_from_parameters}. Only MD5, SCRAM-SHA-256, JWT, and PAM are supported."
        )));
    }

    let transaction_mode = pool.settings.pool_mode == PoolMode::Transaction;
    *prepared_statements_enabled = transaction_mode && pool.prepared_statement_cache.is_some();

    let server_parameters = match pool.get_server_parameters().await {
        Ok(params) => params,
        Err(err) => {
            error!("Failed to retrieve server parameters for database {pool_name}, user {username_from_parameters}: {err:?}");
            error_response(
                write,
                &format!(
                    "Unable to retrieve server parameters for database: {pool_name}, user: {username_from_parameters}. The database server may be unavailable or misconfigured. Please try again later or contact your database administrator."
                ),
                "3D000",
            )
            .await?;
            return Err(err);
        }
    };

    Ok((transaction_mode, server_parameters))
}

/// Authenticate a user with PAM
async fn authenticate_with_pam<S, T>(
    read: &mut S,
    write: &mut T,
    pool: &ConnectionPool,
    username_from_parameters: &str,
) -> Result<(), Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    // pam auth.
    plain_password_challenge(write).await?;
    let password_response = read_password(read).await?;
    let password_response = match vec_to_string(password_response) {
        Ok(p) => p,
        Err(err) => {
            error!("Failed to read PAM password for user {username_from_parameters}: {err}");
            error_response_terminal(
                write,
                "Invalid password format. Password must be valid UTF-8 text.",
                "28P01",
            )
            .await?;
            return Err(err);
        }
    };
    let service = pool.settings.user.auth_pam_service.clone().unwrap();
    match pam_auth(
        service.as_str(),
        username_from_parameters,
        password_response.as_str(),
    ) {
        Ok(_) => (),
        Err(err) => {
            error!(
                "Failed to authenticate user {username_from_parameters} via PAM service {service}: {err}"
            );
            error_response_terminal(
                write,
                "Authentication failed. Please check your username and password.",
                "28P01",
            )
            .await?;
            return Err(Error::AuthError(format!(
                "PAM authentication failed for user: {username_from_parameters} with service: {service}"
            )));
        }
    };

    Ok(())
}

/// Authenticate a user with SCRAM-SHA-256
async fn authenticate_with_scram<S, T>(
    read: &mut S,
    write: &mut T,
    pool_password: &str,
    username_from_parameters: &str,
) -> Result<(), Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    let server_secret = match parse_server_secret(pool_password) {
        Ok(server_secret) => server_secret,
        Err(err) => {
            warn!("Failed to parse SCRAM server secret for user {username_from_parameters}: {err}");
            error_response_terminal(
                write,
                "Server authentication configuration error. Please contact your database administrator.",
                "28P01"
            ).await?;
            return Err(Error::ScramServerError(format!(
                "Failed to parse SCRAM server secret for user: {username_from_parameters}"
            )));
        }
    };
    // scram auth.
    scram_start_challenge(write).await?;
    let first_message = read_password(read).await?;
    let client_first_message = match parse_client_first_message(String::from_utf8_lossy(
        &first_message,
    )) {
        Ok(client_first_message) => client_first_message,
        Err(err) => {
            warn!("Failed to parse SCRAM client first message for user {username_from_parameters}: {err}");
            error_response_terminal(
                    write,
                    "Authentication protocol error. Your client may not support SCRAM authentication properly.",
                    "28P01"
                ).await?;
            return Err(Error::ScramClientError(format!(
                "Failed to parse SCRAM client first message for user: {username_from_parameters}"
            )));
        }
    };
    let server_first_response = prepare_server_first_response(
        client_first_message.nonce.as_str(),
        client_first_message.client_first_bare.as_str(),
        server_secret.salt_base64.as_str(),
        server_secret.iteration,
    );
    scram_server_response(
        write,
        SASL_CONTINUE,
        server_first_response.server_first_bare.as_str(),
    )
    .await?;
    let final_message = read_password(read).await?;
    let client_final_message = match parse_client_final_message(String::from_utf8_lossy(
        &final_message,
    )) {
        Ok(client_final_message) => client_final_message,
        Err(err) => {
            warn!(
                "Failed to parse SCRAM client final message for user {username_from_parameters}: {err}"
            );
            error_response_terminal(
                write,
                "Authentication protocol error. Your client sent an invalid SCRAM final message.",
                "28P01",
            )
            .await?;
            return Err(Error::ScramClientError(format!(
                "Failed to parse SCRAM client final message for user: {username_from_parameters}"
            )));
        }
    };
    let server_final_message = match prepare_server_final_message(
        client_first_message,
        client_final_message,
        server_first_response,
        server_secret.server_key,
        server_secret.stored_key,
    ) {
        Ok(server_final_message) => server_final_message,
        Err(err) => {
            warn!(
                "Failed to prepare SCRAM server final message for user {username_from_parameters}: {err}"
            );
            error_response_terminal(
                write,
                "Authentication failed. Invalid credentials or authentication protocol error.",
                "28P01",
            )
            .await?;
            return Err(Error::ScramServerError(format!(
                "Failed to prepare SCRAM server final message for user: {username_from_parameters}. This may indicate incorrect password or authentication protocol error."
            )));
        }
    };
    scram_server_response(write, SASL_FINAL, server_final_message.as_str()).await?;

    Ok(())
}

/// Authenticate a user with MD5
async fn authenticate_with_md5<S, T>(
    read: &mut S,
    write: &mut T,
    pool_password: &str,
    username_from_parameters: &str,
    pool: &ConnectionPool,
) -> Result<(), Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    // md5 auth.
    let salt = md5_challenge(write).await?;
    let password_response = read_password(read).await?;
    let except_md5_hash = md5_hash_second_pass(pool_password.strip_prefix("md5").unwrap(), &salt);
    if except_md5_hash != password_response {
        error!(
            "MD5 authentication failed for user {} connecting to {}",
            username_from_parameters, pool.address
        );
        error_response_terminal(
            write,
            "Authentication failed. Please check your username and password.",
            "28P01",
        )
        .await?;
        return Err(Error::AuthError(format!(
            "MD5 authentication failed for user: {username_from_parameters}"
        )));
    }

    Ok(())
}

/// Authenticate a user with JWT
async fn authenticate_with_jwt<S, T>(
    read: &mut S,
    write: &mut T,
    jwt_pub_key: String,
    username_from_parameters: &str,
) -> Result<(), Error>
where
    S: AsyncReadExt + Unpin,
    T: AsyncWriteExt + Unpin,
{
    // jwt.
    plain_password_challenge(write).await?;
    let jwt_token_response = read_password(read).await?;
    let jwt_token = match vec_to_string(jwt_token_response) {
        Ok(p) => p,
        Err(err) => {
            error!("Failed to parse JWT token for user {username_from_parameters}: {err}");
            error_response_terminal(
                write,
                "Invalid JWT token format. Token must be valid UTF-8 text.",
                "28P01",
            )
            .await?;
            return Err(Error::JWTValidate(format!(
                "Failed to parse JWT token as UTF-8 for user: {username_from_parameters}"
            )));
        }
    };
    let jwt_user_name = match get_user_name_from_jwt(jwt_pub_key, jwt_token).await {
        Ok(u) => u,
        Err(err) => {
            error!("Failed to validate JWT token for user {username_from_parameters}: {err:?}");
            error_response_terminal(
                write,
                "JWT token validation failed. Please provide a valid token.",
                "28P01",
            )
            .await?;
            return Err(Error::JWTValidate(format!(
                "JWT token validation failed for user: {username_from_parameters}. Token may be expired, malformed, or signed with wrong key."
            )));
        }
    };
    if !jwt_user_name.eq(username_from_parameters) {
        error!("JWT token username mismatch for user {username_from_parameters}: token contains username {jwt_user_name}");
        error_response_terminal(
            write,
            format!("JWT token username mismatch. Token contains username '{jwt_user_name}' but you're trying to connect as '{username_from_parameters}'.").as_str(),
            "28P01"
        ).await?;
        return Err(Error::JWTValidate(format!(
            "JWT token username mismatch: token contains '{jwt_user_name}' but connection requested for '{username_from_parameters}'"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod mocks;
#[cfg(test)]
mod tests;
