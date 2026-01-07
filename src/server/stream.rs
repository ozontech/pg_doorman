use crate::errors::Error;
use crate::messages::{configure_tcp_socket, configure_unix_socket, ssl_request};

use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpStream, UnixStream};

pin_project! {
    #[project = StreamInnerProj]
    #[derive(Debug)]
    pub enum StreamInner {
        TCPPlain {
            #[pin]
            stream: TcpStream,
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
            StreamInnerProj::UnixSocket { stream } => stream.poll_read(cx, buf),
        }
    }
}

impl StreamInner {
    pub fn try_write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            StreamInner::TCPPlain { stream } => stream.try_write(buf),
            StreamInner::UnixSocket { stream } => stream.try_write(buf),
        }
    }
}

pub(crate) async fn create_unix_stream_inner(host: &str, port: u16) -> Result<StreamInner, Error> {
    let stream = match UnixStream::connect(&format!("{host}/.s.PGSQL.{port}")).await {
        Ok(s) => s,
        Err(err) => {
            log::error!("Could not connect to server: {err}");
            return Err(Error::SocketError(format!(
                "Could not connect to server: {err}"
            )));
        }
    };

    configure_unix_socket(&stream);

    Ok(StreamInner::UnixSocket { stream })
}

pub(crate) async fn create_tcp_stream_inner(
    host: &str,
    port: u16,
    tls: bool,
    _verify_server_certificate: bool,
) -> Result<StreamInner, Error> {
    let mut stream = match TcpStream::connect(&format!("{host}:{port}")).await {
        Ok(stream) => stream,
        Err(err) => {
            log::error!("Could not connect to server: {err}");
            return Err(Error::SocketError(format!(
                "Could not connect to server: {err}"
            )));
        }
    };

    // TCP timeouts.
    configure_tcp_socket(&stream);

    let stream = if tls {
        // Request a TLS connection
        ssl_request(&mut stream).await?;

        let response = match stream.read_u8().await {
            Ok(response) => response as char,
            Err(err) => {
                return Err(Error::SocketError(format!(
                    "Failed to read TLS response from server: {err}"
                )));
            }
        };

        match response {
            // Server supports TLS
            'S' => {
                log::error!("Connection to server via tls is not supported");
                return Err(Error::SocketError("Server TLS is unsupported".to_string()));
            }
            // Server does not support TLS
            'N' => StreamInner::TCPPlain { stream },
            // Something else?
            m => {
                return Err(Error::SocketError(format!(
                    "Received unexpected response '{}' (ASCII: {}) during TLS negotiation. Expected 'S' (supports TLS) or 'N' (does not support TLS).",
                    m,
                    m as u8
                )));
            }
        }
    } else {
        StreamInner::TCPPlain { stream }
    };

    Ok(stream)
}
