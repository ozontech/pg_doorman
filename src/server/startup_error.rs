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

/// Locale-independent fallback for `extract_parameter_name`: scan the
/// PG ErrorResponse `M` field for any of the operator-supplied keys
/// pg_doorman actually sent, looking for the standard PG double-quoted
/// form (`"key"`). PG quotes the parameter name in every locale even when
/// the surrounding prose is translated, so a hit on `"name"` is a
/// reliable signal that the failing key is `name`.
///
/// Returns the first match in iteration order of `sent_keys`. Used by
/// `Server::startup` to keep the
/// `pg_doorman_backend_startup_parameter_errors_total` counter usable
/// against PG servers with non-English `lc_messages`.
pub fn match_sent_key_in_message<'a, I>(message: &str, sent_keys: I) -> Option<String>
where
    I: IntoIterator<Item = &'a String>,
{
    for key in sent_keys {
        // Match the GUC name surrounded by double quotes. Bare substring
        // search would false-positive on prose like "key is wrong" when
        // `key` happens to be the parameter name; the quote markers in
        // PG ErrorResponse messages are stable across locales.
        let needle = format!("\"{key}\"");
        if message.contains(&needle) {
            return Some(key.clone());
        }
    }
    None
}

/// Handles error response during server startup.
///
/// Currently unused: `Server::startup` inlines the equivalent logic so it
/// can also surface the failing parameter name into the warn log. Kept
/// in tree as a reference helper for any future startup path that needs
/// the verbatim PG-error-passthrough behaviour.
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

    #[test]
    fn match_sent_key_finds_quoted_key_in_localized_message() {
        // Hypothetical Russian lc_messages output: prose is translated,
        // PG still wraps the parameter name in double quotes.
        let sent = vec!["plan_cache_mode".to_string(), "work_mem".to_string()];
        let msg = r#"параметр "plan_cache_mode" не существует"#;
        assert_eq!(
            match_sent_key_in_message(msg, &sent),
            Some("plan_cache_mode".into())
        );
    }

    #[test]
    fn match_sent_key_returns_none_when_no_quoted_match() {
        let sent = vec!["plan_cache_mode".to_string()];
        // The key appears in prose but not quoted — must not match,
        // otherwise unrelated PG errors mentioning the word would
        // poison the counter.
        let msg = "some unrelated startup error mentions plan_cache_mode somewhere";
        assert!(match_sent_key_in_message(msg, &sent).is_none());
    }

    #[test]
    fn match_sent_key_skips_keys_not_in_message() {
        let sent = vec![
            "first_key".to_string(),
            "second_key".to_string(),
            "third_key".to_string(),
        ];
        let msg = r#"FATAL: invalid value for parameter "second_key""#;
        assert_eq!(
            match_sent_key_in_message(msg, &sent),
            Some("second_key".into())
        );
    }
}
