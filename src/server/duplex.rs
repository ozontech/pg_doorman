//! Independent read/write access to one buffered backend transport.
//!
//! Like `tokio::io::split`, the lock covers a single poll, never an await.
//! Keeping the TLS stream and both buffers together also preserves the
//! transport access needed by idle checks and best-effort Terminate.

use std::future::poll_fn;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, BufStream, ReadBuf};
use tokio::time::timeout;

use crate::errors::Error;
use crate::messages::write_all_flush;

use super::stream::StreamInner;

/// Both sides must finish. A protocol ErrorResponse is handled by `receive`,
/// not returned as an I/O error: the writer still has to deliver Sync.
pub(crate) async fn send_while_receiving<W, F>(
    writer: &mut W,
    messages: &[u8],
    send_timeout: Duration,
    receive: F,
) -> Result<(), Error>
where
    W: AsyncWrite + Unpin,
    F: Future<Output = Result<(), Error>>,
{
    let send = async {
        timeout(send_timeout, write_all_flush(writer, messages))
            .await
            .map_err(|_| Error::FlushTimeout)?
    };
    tokio::try_join!(send, receive)?;
    Ok(())
}

#[derive(Debug)]
pub(crate) struct BackendStream<S = StreamInner> {
    inner: Arc<Mutex<BufStream<S>>>,
}

pub(crate) struct BackendWriter<S> {
    inner: Arc<Mutex<BufStream<S>>>,
}

impl<S: AsyncRead + AsyncWrite> BackendStream<S> {
    pub(crate) fn new(stream: BufStream<S>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(stream)),
        }
    }

    pub(crate) fn writer(&self) -> BackendWriter<S> {
        BackendWriter {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Synchronous transport operations only; the guard cannot escape.
    pub(crate) fn with_inner<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        f(self.inner.lock().get_mut())
    }
}

impl BackendStream {
    pub(crate) async fn readable(&self) -> io::Result<()> {
        poll_fn(|cx| self.with_inner(|stream| stream.poll_read_ready(cx))).await
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for BackendStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner.lock()).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for BackendStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.inner.lock()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner.lock()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner.lock()).poll_shutdown(cx)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for BackendWriter<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.inner.lock()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner.lock()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner.lock()).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::timeout;

    use super::*;

    async fn exchange(concurrent: bool) {
        let (transport, mut peer) = tokio::io::duplex(64);
        let mut stream = BackendStream::new(BufStream::with_capacity(32, 32, transport));
        let mut writer = stream.writer();
        let request = vec![b'Q'; 4096];
        let response = vec![b'D'; 4096];

        let backend = async {
            assert_eq!(peer.read_u8().await.unwrap(), b'Q');
            // Produce a response before consuming the remaining request.
            peer.write_all(&response).await.unwrap();
            let mut rest = vec![0; request.len() - 1];
            peer.read_exact(&mut rest).await.unwrap();
            assert_eq!(rest, request[1..]);
        };
        let frontend = async {
            let receive = async {
                let mut received = vec![0; response.len()];
                stream.read_exact(&mut received).await.unwrap();
                assert_eq!(received, response);
            };
            if concurrent {
                // Exercise the same coordinator used by the client roundtrip.
                send_while_receiving(&mut writer, &request, Duration::from_secs(1), async {
                    receive.await;
                    Ok(())
                })
                .await
                .unwrap();
            } else {
                writer.write_all(&request).await.unwrap();
                writer.flush().await.unwrap();
                receive.await;
            }
        };
        tokio::join!(backend, frontend);
    }

    #[tokio::test]
    async fn reads_responses_while_a_large_request_is_still_being_written() {
        timeout(Duration::from_secs(2), exchange(true))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sequential_exchange_deadlocks_with_the_same_bounded_transport() {
        assert!(timeout(Duration::from_millis(50), exchange(false))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn send_timeout_does_not_limit_query_execution_time() {
        let (mut writer, mut backend) = tokio::io::duplex(64);
        let (ready, wait) = tokio::sync::oneshot::channel();
        let peer = async {
            assert_eq!(backend.read_u8().await.unwrap(), b'Q');
            tokio::time::sleep(Duration::from_millis(50)).await;
            ready.send(()).unwrap();
        };
        let roundtrip = send_while_receiving(&mut writer, b"Q", Duration::from_millis(10), async {
            wait.await.unwrap();
            Ok(())
        });
        let (_, result) = tokio::join!(peer, roundtrip);
        result.unwrap();
    }

    #[tokio::test]
    async fn an_early_response_boundary_does_not_cancel_pending_writes() {
        let (mut writer, mut backend) = tokio::io::duplex(16);
        let request = vec![b'H'; 4096];
        let peer = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut received = vec![0; request.len()];
            backend.read_exact(&mut received).await.unwrap();
            assert_eq!(received, request);
        };
        let roundtrip =
            send_while_receiving(&mut writer, &request, Duration::from_secs(1), async {
                Ok(())
            });
        let (_, result) = tokio::join!(peer, roundtrip);
        result.unwrap();
    }

    #[tokio::test]
    async fn a_stalled_backend_times_out_even_if_the_reader_is_waiting() {
        let (mut writer, _backend) = tokio::io::duplex(16);
        let result = send_while_receiving(
            &mut writer,
            &[b'Q'; 4096],
            Duration::from_millis(20),
            std::future::pending(),
        )
        .await;
        assert!(matches!(result, Err(Error::FlushTimeout)));
    }
}
