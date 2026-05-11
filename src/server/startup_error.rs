use std::mem;

use log::error;
use tokio::io::AsyncReadExt;

use crate::errors::{Error, ServerIdentifier};
use crate::messages::constants::MESSAGE_TERMINATOR;
use crate::messages::PgErrorMsg;

use super::stream::StreamInner;

/// Extract the GUC name from the human-readable M-field of a PG
/// ErrorResponse. Matches messages of the form:
///   `unrecognized configuration parameter "foobar"`
///   `invalid value for parameter "work_mem": "abc"`
///   `permission denied to set parameter "session_preload_libraries"`
///
/// Returns `None` if the message does not contain the literal substring
/// `parameter "..."` (e.g. it is unrelated to a configuration parameter).
///
/// Hand-rolled rather than using `regex` to keep that crate out of the
/// runtime dependency set; the pattern is fixed (`parameter "([^"]+)"`)
/// and trivial to scan for. The same constraint shaped
/// `crate::config::startup_parameters::is_valid_guc_name`.
pub fn extract_parameter_name(message: &str) -> Option<String> {
    const NEEDLE: &str = r#"parameter ""#;
    let start = message.find(NEEDLE)? + NEEDLE.len();
    let rest = &message[start..];
    let end = rest.find('"')?;
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_owned())
}

/// Handles error response during server startup.
///
/// Currently unused: `Server::startup` inlines the equivalent logic so it
/// can also inspect the M-field for the failing parameter name and feed
/// it into the quarantine. Kept in tree as a reference helper for any
/// future startup path that needs the pre-quarantine behavior verbatim.
#[allow(dead_code)]
pub(crate) async fn handle_startup_error(
    stream: &mut StreamInner,
    len: i32,
    server_identifier: &ServerIdentifier,
) -> Result<(), Error> {
    let error_code = stream.read_u8().await.map_err(|_| {
        Error::ServerStartupError("error code message".into(), server_identifier.clone())
    })?;

    match error_code {
        MESSAGE_TERMINATOR => Err(Error::ServerError),
        _ => {
            if (len as usize) < 2 * mem::size_of::<u32>() {
                return Err(Error::ServerStartupError(
                    "startup error message too short to parse".to_string(),
                    server_identifier.clone(),
                ));
            }

            let mut error = vec![0u8; len as usize - 2 * mem::size_of::<u32>()];
            stream.read_exact(&mut error).await.map_err(|err| {
                Error::ServerStartupError(
                    format!("failed to parse startup error details: {err:?}"),
                    server_identifier.clone(),
                )
            })?;

            match PgErrorMsg::parse(&error) {
                Ok(f) => {
                    error!(
                        "[{}@{}] startup error: severity={}, code={}, message={}",
                        server_identifier.username,
                        server_identifier.pool_name,
                        f.severity,
                        f.code,
                        f.message
                    );
                    if f.code.starts_with("57P") {
                        Err(Error::ServerUnavailableError(
                            f.message,
                            server_identifier.clone(),
                        ))
                    } else {
                        Err(Error::ServerStartupError(
                            f.message,
                            server_identifier.clone(),
                        ))
                    }
                }
                Err(err) => {
                    error!(
                        "[{}@{}] startup error: could not parse: {err}",
                        server_identifier.username, server_identifier.pool_name
                    );
                    Err(Error::ServerStartupError(
                        format!("failed to parse startup error details: {err:?}"),
                        server_identifier.clone(),
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod parameter_extractor_tests {
    use super::*;

    #[test]
    fn unknown_parameter_extracted() {
        assert_eq!(
            extract_parameter_name(r#"unrecognized configuration parameter "foobar""#),
            Some("foobar".into())
        );
    }

    #[test]
    fn invalid_value_extracted() {
        assert_eq!(
            extract_parameter_name(r#"invalid value for parameter "work_mem": "abc""#),
            Some("work_mem".into())
        );
    }

    #[test]
    fn permission_denied_extracted() {
        assert_eq!(
            extract_parameter_name(
                r#"permission denied to set parameter "session_preload_libraries""#
            ),
            Some("session_preload_libraries".into())
        );
    }

    #[test]
    fn no_quoted_parameter_returns_none() {
        assert_eq!(extract_parameter_name("connection refused by peer"), None);
    }

    #[test]
    fn namespaced_parameter_extracted() {
        assert_eq!(
            extract_parameter_name(
                r#"unrecognized configuration parameter "auto_explain.log_min_duration""#
            ),
            Some("auto_explain.log_min_duration".into())
        );
    }
}
