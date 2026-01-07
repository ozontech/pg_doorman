use std::mem;

use log::error;
use tokio::io::AsyncReadExt;

use crate::errors::{Error, ServerIdentifier};
use crate::messages::constants::MESSAGE_TERMINATOR;
use crate::messages::PgErrorMsg;

use super::stream::StreamInner;

/// Handles error response during server startup.
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
                    "while create new connection to postgresql received error, but it's too small"
                        .to_string(),
                    server_identifier.clone(),
                ));
            }

            let mut error = vec![0u8; len as usize - 2 * mem::size_of::<u32>()];
            stream.read_exact(&mut error).await.map_err(|err| {
                Error::ServerStartupError(
                    format!("while create new connection to postgresql received error, but can't read it: {err:?}"),
                    server_identifier.clone(),
                )
            })?;

            match PgErrorMsg::parse(&error) {
                Ok(f) => {
                    error!(
                        "Get server error - {} {}: {}",
                        f.severity, f.code, f.message
                    );
                    Err(Error::ServerStartupError(
                        f.message,
                        server_identifier.clone(),
                    ))
                }
                Err(err) => {
                    error!("Get unparsed server error: {err:?}");
                    Err(Error::ServerStartupError(
                        format!("while create new connection to postgresql received error, but can't read it: {err:?}"),
                        server_identifier.clone(),
                    ))
                }
            }
        }
    }
}
