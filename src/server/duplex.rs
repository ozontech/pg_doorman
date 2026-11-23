//! Independent read/write access to one buffered backend transport.
//!
//! Like `tokio::io::split`, the lock covers a single poll, never an await.
//! Keeping the TLS stream and both buffers together also preserves the
//! transport access needed by idle checks and best-effort Terminate.

use std::future::poll_fn;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufStream, ReadBuf};

use crate::errors::Error;

use super::stream::StreamInner;

/// Both sides must finish. A protocol ErrorResponse is handled by `receive`,
/// not returned as an I/O error: the writer still has to deliver Sync.
/// `send_complete` lets the reader drain after a client disconnect only when
/// the entire batch has been flushed. AtomicBool keeps the future Send.
///
/// There is no deadline for the batch as a whole: PostgreSQL can be executing
/// earlier commands while the remaining request waits for socket capacity.
/// `write_idle_timeout` bounds only the time the writer makes no progress;
/// it fails the exchange with `Error::FlushTimeout`. `bytes_sent` reports how
/// much of `messages` the transport had accepted, for diagnostics.
pub(crate) async fn send_while_receiving<W, F>(
    writer: &mut W,
    messages: &[u8],
    send_complete: &AtomicBool,
    write_idle_timeout: Option<Duration>,
    bytes_sent: &AtomicUsize,
    receive: F,
) -> Result<(), Error>
where
    W: AsyncWrite + Unpin,
    F: Future<Output = Result<(), Error>>,
{
    send_complete.store(false, Ordering::Relaxed);
    bytes_sent.store(0, Ordering::Relaxed);
    let send = async {
        write_all_flush_idle(writer, messages, write_idle_timeout, bytes_sent).await?;
        send_complete.store(true, Ordering::Relaxed);
        Ok::<(), Error>(())
    };
    tokio::try_join!(send, receive)?;
    Ok(())
}

/// `write_all` + `flush` that gives up with `Error::FlushTimeout` when the
/// writer accepts nothing for `idle_timeout`. The limit applies to each
/// `write` call, which returns as soon as the transport accepts any bytes, so
/// it bounds a stalled peer, not the transfer size.
async fn write_all_flush_idle<W>(
    writer: &mut W,
    buf: &[u8],
    idle_timeout: Option<Duration>,
    bytes_sent: &AtomicUsize,
) -> Result<(), Error>
where
    W: AsyncWrite + Unpin,
{
    let mut written = 0;
    while written < buf.len() {
        let accepted = within_idle_timeout(idle_timeout, writer.write(&buf[written..]))
            .await?
            .map_err(|err| Error::SocketError(format!("Error writing to socket: {err:?}")))?;
        if accepted == 0 {
            return Err(Error::SocketError(
                "Error writing to socket: write returned zero bytes".to_string(),
            ));
        }
        written += accepted;
        bytes_sent.store(written, Ordering::Relaxed);
    }
    within_idle_timeout(idle_timeout, writer.flush())
        .await?
        .map_err(|err| Error::SocketError(format!("Error flushing socket: {err:?}")))
}

/// Awaits one write step; `None` waits for the peer indefinitely.
async fn within_idle_timeout<T>(
    idle_timeout: Option<Duration>,
    step: impl Future<Output = T>,
) -> Result<T, Error> {
    match idle_timeout {
        Some(limit) => tokio::time::timeout(limit, step)
            .await
            .map_err(|_| Error::FlushTimeout),
        None => Ok(step.await),
    }
}

/// Wraps the client-facing writer for the duration of one exchange and
/// records whether its last write was left pending. When the backend write
/// stalls, this tells a client that stopped reading replies (backpressure
/// reaches PostgreSQL through pg_doorman) from a backend that stopped reading.
pub(crate) struct ClientWriteProbe<'a, W> {
    inner: &'a mut W,
    blocked: &'a AtomicBool,
}

impl<'a, W> ClientWriteProbe<'a, W> {
    pub(crate) fn new(inner: &'a mut W, blocked: &'a AtomicBool) -> Self {
        Self { inner, blocked }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ClientWriteProbe<'_, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut *this.inner).poll_write(cx, buf);
        this.blocked.store(result.is_pending(), Ordering::Relaxed);
        result
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut *this.inner).poll_flush(cx);
        this.blocked.store(result.is_pending(), Ordering::Relaxed);
        result
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.get_mut().inner).poll_shutdown(cx)
    }
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
    use tokio::time::{timeout, Instant};

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
                send_while_receiving(
                    &mut writer,
                    &request,
                    &AtomicBool::new(false),
                    None,
                    &AtomicUsize::new(0),
                    async {
                        receive.await;
                        Ok(())
                    },
                )
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
    async fn waits_for_the_response_after_the_send_completes() {
        let (mut writer, mut backend) = tokio::io::duplex(64);
        let (ready, wait) = tokio::sync::oneshot::channel();
        let peer = async {
            assert_eq!(backend.read_u8().await.unwrap(), b'Q');
            tokio::time::sleep(Duration::from_millis(50)).await;
            ready.send(()).unwrap();
        };
        let send_complete = AtomicBool::new(false);
        let bytes_sent = AtomicUsize::new(0);
        let roundtrip = send_while_receiving(
            &mut writer,
            b"Q",
            &send_complete,
            None,
            &bytes_sent,
            async {
                wait.await.unwrap();
                Ok(())
            },
        );
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
        let send_complete = AtomicBool::new(false);
        let bytes_sent = AtomicUsize::new(0);
        let roundtrip = send_while_receiving(
            &mut writer,
            &request,
            &send_complete,
            None,
            &bytes_sent,
            async { Ok(()) },
        );
        let (_, result) = tokio::join!(peer, roundtrip);
        result.unwrap();
    }

    #[tokio::test]
    async fn a_reader_error_cancels_a_blocked_writer() {
        let (mut writer, _backend) = tokio::io::duplex(16);
        let send_complete = AtomicBool::new(false);
        let result = timeout(
            Duration::from_secs(1),
            send_while_receiving(
                &mut writer,
                &[b'Q'; 4096],
                &send_complete,
                None,
                &AtomicUsize::new(0),
                async { Err(Error::SocketError("backend read failed".into())) },
            ),
        )
        .await
        .expect("reader error did not cancel the blocked writer");
        assert!(matches!(result, Err(Error::SocketError(_))));
        assert!(!send_complete.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn a_writer_error_cancels_a_waiting_reader() {
        let (mut writer, backend) = tokio::io::duplex(16);
        drop(backend);
        let send_complete = AtomicBool::new(false);
        let result = timeout(
            Duration::from_secs(1),
            send_while_receiving(
                &mut writer,
                &[b'Q'; 4096],
                &send_complete,
                None,
                &AtomicUsize::new(0),
                std::future::pending(),
            ),
        )
        .await
        .expect("writer error did not cancel the waiting reader");
        assert!(matches!(result, Err(Error::SocketError(_))));
        assert!(!send_complete.load(Ordering::Relaxed));
    }

    // ---- write idle timeout -------------------------------------------------
    // Real time with lower-bound assertions only: tokio's paused clock needs
    // the `test-util` feature, which this crate does not enable.

    const IDLE: Duration = Duration::from_millis(300);

    #[tokio::test]
    async fn a_stalled_writer_fails_with_flush_timeout() {
        // The peer never reads: 16 bytes fit, the rest can never be written.
        let (mut writer, _backend) = tokio::io::duplex(16);
        let send_complete = AtomicBool::new(false);
        let bytes_sent = AtomicUsize::new(0);
        let started = Instant::now();
        let result = send_while_receiving(
            &mut writer,
            &[b'Q'; 4096],
            &send_complete,
            Some(IDLE),
            &bytes_sent,
            std::future::pending(),
        )
        .await;
        assert!(matches!(result, Err(Error::FlushTimeout)));
        assert!(started.elapsed() >= IDLE);
        assert_eq!(bytes_sent.load(Ordering::Relaxed), 16);
        assert!(!send_complete.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn write_progress_restarts_the_idle_countdown() {
        // The peer takes 16 bytes every 30 ms: the transfer lasts longer than
        // the idle limit, but no single stall comes close to it.
        let (mut writer, mut backend) = tokio::io::duplex(16);
        let request = vec![b'Q'; 16 * 16];
        let peer = async {
            let mut chunk = [0u8; 16];
            for _ in 0..request.len() / chunk.len() {
                tokio::time::sleep(Duration::from_millis(30)).await;
                backend.read_exact(&mut chunk).await.unwrap();
            }
        };
        let send_complete = AtomicBool::new(false);
        let bytes_sent = AtomicUsize::new(0);
        let started = Instant::now();
        let roundtrip = send_while_receiving(
            &mut writer,
            &request,
            &send_complete,
            Some(IDLE),
            &bytes_sent,
            async { Ok(()) },
        );
        let (_, result) = tokio::join!(peer, roundtrip);
        result.unwrap();
        assert!(started.elapsed() > IDLE);
        assert_eq!(bytes_sent.load(Ordering::Relaxed), request.len());
        assert!(send_complete.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn the_countdown_runs_from_the_last_progress_not_from_the_start() {
        // One chunk is accepted after a delay, then the peer stops for good:
        // the timeout must fire a full idle period after that chunk.
        let delay = Duration::from_millis(200);
        let (mut writer, mut backend) = tokio::io::duplex(16);
        let peer = async {
            let mut chunk = [0u8; 16];
            tokio::time::sleep(delay).await;
            backend.read_exact(&mut chunk).await.unwrap();
            std::future::pending::<()>().await;
        };
        let send_complete = AtomicBool::new(false);
        let bytes_sent = AtomicUsize::new(0);
        let started = Instant::now();
        let roundtrip = send_while_receiving(
            &mut writer,
            &[b'Q'; 4096],
            &send_complete,
            Some(IDLE),
            &bytes_sent,
            std::future::pending(),
        );
        let result = tokio::select! {
            result = roundtrip => result,
            _ = peer => unreachable!(),
        };
        assert!(matches!(result, Err(Error::FlushTimeout)));
        assert!(started.elapsed() >= delay + IDLE);
        assert_eq!(bytes_sent.load(Ordering::Relaxed), 32);
    }

    #[tokio::test]
    async fn a_disabled_timeout_waits_for_the_peer() {
        // The peer stays silent for longer than IDLE; with no timeout
        // configured the writer simply waits.
        let (mut writer, mut backend) = tokio::io::duplex(16);
        let request = vec![b'Q'; 64];
        let peer = async {
            tokio::time::sleep(IDLE * 2).await;
            let mut received = vec![0; request.len()];
            backend.read_exact(&mut received).await.unwrap();
        };
        let send_complete = AtomicBool::new(false);
        let bytes_sent = AtomicUsize::new(0);
        let roundtrip = send_while_receiving(
            &mut writer,
            &request,
            &send_complete,
            None,
            &bytes_sent,
            async { Ok(()) },
        );
        let (_, result) = tokio::join!(peer, roundtrip);
        result.unwrap();
        assert!(send_complete.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn a_slow_response_is_not_a_write_stall() {
        // The request fits the transport, so the writer is done at once; a
        // backend that answers later than IDLE must not trip the timeout.
        let (mut writer, _backend) = tokio::io::duplex(64);
        let send_complete = AtomicBool::new(false);
        let result = send_while_receiving(
            &mut writer,
            b"Q",
            &send_complete,
            Some(IDLE),
            &AtomicUsize::new(0),
            async {
                tokio::time::sleep(IDLE * 2).await;
                Ok(())
            },
        )
        .await;
        result.unwrap();
    }

    #[tokio::test]
    async fn the_client_probe_reports_a_client_that_stopped_reading() {
        let (mut client, mut peer) = tokio::io::duplex(16);
        let blocked = AtomicBool::new(false);
        {
            let mut probe = ClientWriteProbe::new(&mut client, &blocked);
            probe.write_all(&[b'D'; 16]).await.unwrap();
            assert!(!blocked.load(Ordering::Relaxed));
            // The transport is full now and nobody reads it.
            let stuck = timeout(Duration::from_millis(50), probe.write_all(&[b'D'; 16])).await;
            assert!(stuck.is_err());
            assert!(blocked.load(Ordering::Relaxed));
        }
        // Once the client reads again, the next write clears the flag.
        let mut drained = [0u8; 16];
        peer.read_exact(&mut drained).await.unwrap();
        let mut probe = ClientWriteProbe::new(&mut client, &blocked);
        probe.write_all(&[b'D'; 8]).await.unwrap();
        assert!(!blocked.load(Ordering::Relaxed));
    }
}
