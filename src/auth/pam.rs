// Standard library imports

// External crate imports
use log::error;
#[cfg(all(target_os = "linux", feature = "pam"))]
use pam_client::conv_mock::Conversation;
#[cfg(all(target_os = "linux", feature = "pam"))]
use pam_client::{Context, Flag};

// Internal crate imports
use crate::errors::Error;

/// Helper function to log an error message and return an AuthError
fn auth_error<T, E: std::fmt::Display>(msg: &str, err: E) -> Result<T, Error> {
    let error_msg = format!("{msg}: {err}");
    error!("{error_msg}");
    Err(Error::AuthError(error_msg))
}

#[cfg(all(target_os = "linux", not(feature = "pam")))]
pub fn pam_auth(_service: &str, _username: &str, _password: &str) -> Result<(), Error> {
    let msg = "PAM authentication failed: This build was compiled without PAM support. Please recompile with the 'pam' feature enabled or use a different authentication method.";
    auth_error(msg, Error::ServerError)
}

#[cfg(all(target_os = "linux", feature = "pam"))]
pub fn pam_auth(service: &str, username: &str, password: &str) -> Result<(), Error> {
    let mut context = match Context::new(
        service,
        None,
        Conversation::with_credentials(username, password),
    ) {
        Ok(c) => c,
        Err(err) => {
            return auth_error(
                &format!(
                "Failed to initialize PAM context for service '{service}' and user '{username}'"
            ),
                err,
            )
        }
    };

    if let Err(err) = context.authenticate(Flag::NONE) {
        return auth_error(
            &format!("PAM authentication failed for user '{username}' with service '{service}'"),
            err,
        );
    }

    if let Err(err) = context.acct_mgmt(Flag::NONE) {
        return auth_error(
            &format!(
                "PAM account validation failed for user '{username}' with service '{service}'"
            ),
            err,
        );
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn pam_auth(_service: &str, _username: &str, _password: &str) -> Result<(), Error> {
    let msg = "PAM authentication failed: PAM is only supported on Linux platforms. Current platform is not supported. Please use a different authentication method.";
    auth_error(msg, Error::ServerError)
}
