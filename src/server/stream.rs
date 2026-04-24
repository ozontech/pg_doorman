use std::time::Instant;

use crate::config::tls::ServerTlsConfig;
use crate::errors::Error;
use crate::messages::{configure_tcp_socket, configure_unix_socket, ssl_request};

use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpStream, UnixStream};
use tokio_native_tls::TlsStream;

pin_project! {
    #[project = StreamInnerProj]
    #[derive(Debug)]
    pub enum StreamInner {
        TCPPlain {
            #[pin]
            stream: TcpStream,
        },
        TCPTls {
            #[pin]
            stream: TlsStream<TcpStream>,
        },
        UnixSocket {
            #[pin]
            stream: UnixStream,
        },
    }
}

impl AsyncWrite for StreamInner {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let this = self.project();
        match this {
            StreamInnerProj::TCPPlain { stream } => stream.poll_write(cx, buf),
            StreamInnerProj::TCPTls { stream } => stream.poll_write(cx, buf),
            StreamInnerProj::UnixSocket { stream } => stream.poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let this = self.project();
        match this {
            StreamInnerProj::TCPPlain { stream } => stream.poll_flush(cx),
            StreamInnerProj::TCPTls { stream } => stream.poll_flush(cx),
            StreamInnerProj::UnixSocket { stream } => stream.poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let this = self.project();
        match this {
            StreamInnerProj::TCPPlain { stream } => stream.poll_shutdown(cx),
            StreamInnerProj::TCPTls { stream } => stream.poll_shutdown(cx),
            StreamInnerProj::UnixSocket { stream } => stream.poll_shutdown(cx),
        }
    }
}

impl AsyncRead for StreamInner {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.project();
        match this {
            StreamInnerProj::TCPPlain { stream } => stream.poll_read(cx, buf),
            StreamInnerProj::TCPTls { stream } => stream.poll_read(cx, buf),
            StreamInnerProj::UnixSocket { stream } => stream.poll_read(cx, buf),
        }
    }
}

impl StreamInner {
    pub fn try_write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            StreamInner::TCPPlain { stream } => stream.try_write(buf),
            StreamInner::TCPTls { stream } => {
                let waker = std::task::Waker::noop();
                let mut cx = std::task::Context::from_waker(waker);
                match std::pin::Pin::new(stream).poll_write(&mut cx, buf) {
                    std::task::Poll::Ready(result) => result,
                    std::task::Poll::Pending => {
                        Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
                    }
                }
            }
            StreamInner::UnixSocket { stream } => stream.try_write(buf),
        }
    }

    /// Async write that properly handles TLS back-pressure.
    /// Use this instead of try_write() when in an async context
    /// (e.g., server authentication). try_write() uses a noop waker
    /// for TLS which silently fails on Pending — this method awaits
    /// until the full buffer is written.
    pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        match self {
            StreamInner::TCPPlain { stream } => stream.write_all(buf).await,
            StreamInner::TCPTls { stream } => stream.write_all(buf).await,
            StreamInner::UnixSocket { stream } => stream.write_all(buf).await,
        }
    }

    /// Waits until the server socket becomes readable (data or EOF/error).
    /// Cancel-safe: no data is consumed, only readiness notification.
    pub async fn readable(&self) -> std::io::Result<()> {
        match self {
            StreamInner::TCPPlain { stream } => stream.readable().await,
            StreamInner::TCPTls { stream } => stream.get_ref().get_ref().get_ref().readable().await,
            StreamInner::UnixSocket { stream } => stream.readable().await,
        }
    }

    /// Returns true if this stream uses TLS encryption.
    pub fn is_tls(&self) -> bool {
        matches!(self, StreamInner::TCPTls { .. })
    }

    /// Non-blocking read attempt on the raw socket (bypasses BufStream).
    /// Used to verify that `readable()` readiness is genuine, not spurious
    /// from BufStream buffering. Returns WouldBlock if no data available.
    pub fn try_read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            StreamInner::TCPPlain { stream } => stream.try_read(buf),
            StreamInner::TCPTls { stream } => stream.get_ref().get_ref().get_ref().try_read(buf),
            StreamInner::UnixSocket { stream } => stream.try_read(buf),
        }
    }
}

pub(crate) async fn create_unix_stream_inner(host: &str, port: u16) -> Result<StreamInner, Error> {
    let stream = match UnixStream::connect(&format!("{host}/.s.PGSQL.{port}")).await {
        Ok(s) => s,
        Err(err) => {
            log::error!("Failed to connect to Unix socket {host}:{port}: {err}");
            return Err(Error::SocketError(format!(
                "Failed to connect to Unix socket {host}:{port}: {err}"
            )));
        }
    };

    configure_unix_socket(&stream);

    Ok(StreamInner::UnixSocket { stream })
}

pub(crate) async fn create_tcp_stream_inner(
    host: &str,
    port: u16,
    server_tls: &ServerTlsConfig,
    pool_name: &str,
) -> Result<StreamInner, Error> {
    let mut stream = match TcpStream::connect(&format!("{host}:{port}")).await {
        Ok(stream) => stream,
        Err(err) => {
            log::error!("Failed to connect to TCP {host}:{port}: {err}");
            return Err(Error::SocketError(format!(
                "Could not connect to {host}:{port}: {err}"
            )));
        }
    };

    configure_tcp_socket(&stream);

    if !server_tls.mode.sends_ssl_request() {
        log::debug!(
            "tls negotiation skipped, server_tls_mode={} host={host} port={port}",
            server_tls.mode
        );
        return Ok(StreamInner::TCPPlain { stream });
    }

    log::debug!(
        "tls negotiation started, server_tls_mode={} host={host} port={port}",
        server_tls.mode
    );
    ssl_request(&mut stream).await?;

    let response = match stream.read_u8().await {
        Ok(response) => {
            log::debug!(
                "tls negotiation response={} (0x{:02x}) host={host} port={port}",
                response as char,
                response
            );
            response as char
        }
        Err(err) => {
            log::error!("Failed to read TLS response from {host}:{port}: {err}");
            return Err(Error::SocketError(format!(
                "Failed to read TLS response from {host}:{port}: {err}"
            )));
        }
    };

    match response {
        'S' => {
            let connector = server_tls.connector.as_ref().ok_or_else(|| {
                Error::SocketError(format!(
                    "tls connector not configured but server accepted tls, host={host} port={port}"
                ))
            })?;

            let start = Instant::now();
            match connector.connect(host, stream).await {
                Ok(tls_stream) => {
                    let elapsed = start.elapsed();
                    log::info!(
                        "tls connection established, host={host} port={port} server_tls_mode={} handshake_ms={:.1}",
                        server_tls.mode,
                        elapsed.as_secs_f64() * 1000.0
                    );
                    crate::prometheus::SHOW_SERVER_TLS_HANDSHAKE_DURATION
                        .with_label_values(&[pool_name])
                        .observe(elapsed.as_secs_f64());
                    Ok(StreamInner::TCPTls { stream: tls_stream })
                }
                // We do NOT retry on a new plain TCP socket when TLS handshake
                // fails after server responded 'S'. The TCP connection is already
                // consumed by the partial handshake.
                Err(err) => {
                    let elapsed = start.elapsed();
                    log::error!(
                        "tls handshake failed, host={host} port={port} server_tls_mode={} handshake_ms={:.1}: {err}",
                        server_tls.mode,
                        elapsed.as_secs_f64() * 1000.0
                    );
                    crate::prometheus::SHOW_SERVER_TLS_HANDSHAKE_ERRORS
                        .with_label_values(&[pool_name])
                        .inc();
                    Err(Error::SocketError(format!(
                        "tls handshake failed, host={host} port={port}: {err}"
                    )))
                }
            }
        }
        'N' => {
            if server_tls.mode.requires_tls() {
                log::error!(
                    "tls required but server does not support tls, host={host} port={port} server_tls_mode={}",
                    server_tls.mode
                );
                crate::prometheus::SHOW_SERVER_TLS_HANDSHAKE_ERRORS
                    .with_label_values(&[pool_name])
                    .inc();
                Err(Error::SocketError(format!(
                    "tls required but server does not support tls, host={host} port={port} server_tls_mode={}",
                    server_tls.mode
                )))
            } else {
                log::info!(
                    "tls not supported by server, falling back to plain tcp, host={host} port={port} server_tls_mode={}",
                    server_tls.mode
                );
                Ok(StreamInner::TCPPlain { stream })
            }
        }
        'E' => Err(Error::SocketError(format!(
            "server sent error response to ssl request, \
             likely does not support ssl or is not a postgresql server, \
             host={host} port={port}"
        ))),
        other => Err(Error::SocketError(format!(
            "unexpected tls negotiation response={} (0x{:02x}) host={host} port={port}",
            other, other as u8
        ))),
    }
}
