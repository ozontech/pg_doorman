use log::error;

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

    /// Waits until the server socket becomes readable (data or EOF/error).
    /// Cancel-safe: no data is consumed, only readiness notification.
    pub async fn readable(&self) -> std::io::Result<()> {
        match self {
            StreamInner::TCPPlain { stream } => stream.readable().await,
            StreamInner::TCPTls { stream } => stream.get_ref().get_ref().get_ref().readable().await,
            StreamInner::UnixSocket { stream } => stream.readable().await,
        }
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
            error!("Failed to connect to Unix socket {host}:{port}: {err}");
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
) -> Result<StreamInner, Error> {
    let mut stream = match TcpStream::connect(&format!("{host}:{port}")).await {
        Ok(stream) => stream,
        Err(err) => {
            error!("Failed to connect to TCP {host}:{port}: {err}");
            return Err(Error::SocketError(format!(
                "Could not connect to {host}:{port}: {err}"
            )));
        }
    };

    configure_tcp_socket(&stream);

    if !server_tls.mode.sends_ssl_request() {
        return Ok(StreamInner::TCPPlain { stream });
    }

    ssl_request(&mut stream).await?;

    let response = match stream.read_u8().await {
        Ok(response) => response as char,
        Err(err) => {
            return Err(Error::SocketError(format!(
                "Failed to read TLS response from {host}:{port}: {err}"
            )));
        }
    };

    match response {
        'S' => {
            let connector = server_tls.connector.as_ref().ok_or_else(|| {
                Error::SocketError(format!(
                    "Server {host}:{port} supports TLS but no TLS connector configured"
                ))
            })?;

            match connector.connect(host, stream).await {
                Ok(tls_stream) => {
                    log::info!(
                        "TLS connection established to {host}:{port} (mode: {})",
                        server_tls.mode
                    );
                    Ok(StreamInner::TCPTls { stream: tls_stream })
                }
                Err(err) => {
                    error!(
                        "TLS handshake failed with {host}:{port} (mode: {}): {err}",
                        server_tls.mode
                    );
                    Err(Error::SocketError(format!(
                        "TLS handshake failed with {host}:{port}: {err}"
                    )))
                }
            }
        }
        'N' => {
            if server_tls.mode.requires_tls() {
                error!(
                    "Server {host}:{port} does not support TLS but server_tls_mode is {}",
                    server_tls.mode
                );
                Err(Error::SocketError(format!(
                    "Server {host}:{port} does not support TLS but server_tls_mode is {}",
                    server_tls.mode
                )))
            } else {
                log::info!(
                    "Server {host}:{port} does not support TLS, using plain TCP (mode: {})",
                    server_tls.mode
                );
                Ok(StreamInner::TCPPlain { stream })
            }
        }
        other => Err(Error::SocketError(format!(
            "Unexpected TLS response '{}' (ASCII: {}) from {host}:{port}",
            other, other as u8
        ))),
    }
}
