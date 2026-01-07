use crate::client::core::Client;
use crate::errors::Error;
use crate::messages::error_response;

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    /// Helper to send error response and return the error
    pub(crate) async fn send_error_response(
        &mut self,
        message: &str,
        code: &str,
        err: Error,
    ) -> Result<(), Error> {
        error_response(&mut self.write, message, code).await?;
        Err(err)
    }

    pub(crate) async fn process_error(&mut self, err: Error) -> Result<(), Error> {
        match err {
            Error::MaxMessageSize => {
                self.send_error_response(
                    "Message exceeds maximum allowed size. Please reduce the size of your query or data.",
                    "53200",
                    err,
                ).await
            }
            Error::CurrentMemoryUsage => {
                self.send_error_response(
                    "Server is temporarily out of memory. Please try again later or reduce the size of your query.",
                    "53200",
                    err,
                ).await
            }
            Error::SocketError(ref msg) => {
                let message = format!("Network connection error: {msg}. Please check your network connection.");
                self.send_error_response(&message, "08006", err).await
            }
            Error::QueryWaitTimeout => {
                self.send_error_response(
                    "Query wait timed out. The server may be overloaded.",
                    "57014",
                    err,
                ).await
            }
            Error::AllServersDown => {
                self.send_error_response(
                    "All database servers are currently unavailable. Please try again later.",
                    "08006",
                    err,
                ).await
            }
            Error::ShuttingDown => {
                self.send_error_response(
                    "Connection pooler is shutting down. Please reconnect in a few moments.",
                    "58006",
                    err,
                ).await
            }
            Error::FlushTimeout => {
                self.send_error_response(
                    "Timeout while sending data to client. Please check your network connection.",
                    "08006",
                    err,
                ).await
            }
            Error::ProxyTimeout => {
                self.send_error_response(
                    "Proxy operation timed out. Please try again later.",
                    "08006",
                    err,
                ).await
            }
            _ => Err(err),
        }
    }
}
