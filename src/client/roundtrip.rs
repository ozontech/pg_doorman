use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::AsyncWrite;
use tokio::time::timeout;

use crate::messages::error_response_terminal;

pub(super) const BACKEND_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINAL_ERROR_TIMEOUT: Duration = Duration::from_secs(1);
const BACKEND_FLUSH_TIMEOUT_MESSAGE: &str =
    "connection to PostgreSQL backend closed (flush timeout: backend did not accept data within the timeout period)";

/// Conservatively track whether cancellation could leave a partial backend
/// message on the client connection. Cleared only at a known frame boundary.
pub(super) struct ResponseWriter<'a, W> {
    pub(super) inner: &'a mut W,
    pub(super) in_progress: &'a mut bool,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ResponseWriter<'_, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut *this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = result {
            if written > 0 {
                *this.in_progress = true;
            }
        }
        result
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.get_mut().inner).poll_shutdown(cx)
    }
}

pub(super) async fn send_flush_timeout_error<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response_in_progress: bool,
) {
    if response_in_progress {
        // A terminal ErrorResponse cannot be inserted into a partial DataRow
        // (or another message). Closing the connection is the only safe exit.
        return;
    }
    let _ = timeout(
        TERMINAL_ERROR_TIMEOUT,
        error_response_terminal(writer, BACKEND_FLUSH_TIMEOUT_MESSAGE, "58006"),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::utils::buffering_writer::BufferingWriter;

    use super::*;

    #[tokio::test]
    async fn timeout_at_frame_boundary_reports_backend_and_keeps_sqlstate() {
        let mut bytes = BytesMut::new();
        send_flush_timeout_error(&mut BufferingWriter::new(&mut bytes), false).await;
        assert_eq!(bytes[0], b'E');
        let len = i32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), len + 1); // No synthetic ReadyForQuery.
        assert!(bytes.windows(7).any(|w| w == b"C58006\0"));
        let message = String::from_utf8_lossy(&bytes);
        assert!(message.contains("PostgreSQL backend"));
        assert!(!message.contains("pooler is shut down"));
    }

    #[tokio::test]
    async fn cancellation_during_a_message_never_appends_an_error_frame() {
        let (mut client, mut application) = tokio::io::duplex(16);
        let mut in_progress = false;
        let body = [b'D'; 128];
        {
            let mut writer = ResponseWriter {
                inner: &mut client,
                in_progress: &mut in_progress,
            };
            assert!(timeout(Duration::from_millis(20), writer.write_all(&body))
                .await
                .is_err());
        }
        assert!(in_progress);
        send_flush_timeout_error(&mut client, in_progress).await;
        drop(client);
        let mut received = Vec::new();
        application.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, body[..16]);
    }

    #[tokio::test]
    async fn no_bytes_written_leaves_a_safe_error_boundary() {
        let (mut client, _application) = tokio::io::duplex(1);
        client.write_all(b"x").await.unwrap();
        let mut in_progress = false;
        let mut writer = ResponseWriter {
            inner: &mut client,
            in_progress: &mut in_progress,
        };
        assert!(timeout(Duration::from_millis(20), writer.write_all(b"y"))
            .await
            .is_err());
        assert!(!in_progress);
    }

    #[tokio::test]
    async fn terminal_error_does_not_wait_forever_for_a_stalled_client() {
        let (mut client, _application) = tokio::io::duplex(1);
        timeout(
            Duration::from_secs(2),
            send_flush_timeout_error(&mut client, false),
        )
        .await
        .unwrap();
    }
}
