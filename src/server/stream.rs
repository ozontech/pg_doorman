use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
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

    /// Move readiness registration to the current reactor without reconnecting.
    /// Buffered bytes and, for TLS, the negotiated session stay in place.
    pub async fn rebind_to_current_worker(&mut self) -> io::Result<()> {
        match self {
            StreamInner::TCPPlain { stream } => rebind_tcp(stream),
            StreamInner::TCPTls { stream } => {
                rebind_tcp(stream.get_mut().get_mut().get_mut())?;
                // native-tls may buffer the first write unless the new registration
                // has observed writability; without this the peer receives nothing.
                stream.get_mut().get_mut().get_mut().writable().await
            }
            StreamInner::UnixSocket { stream } => rebind_unix(stream),
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

fn duplicate_socket(fd: std::os::fd::RawFd) -> io::Result<OwnedFd> {
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    let duplicated = unsafe { OwnedFd::from_raw_fd(duplicated) };
    let flags = unsafe { libc::fcntl(duplicated.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe {
            libc::fcntl(
                duplicated.as_raw_fd(),
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            )
        } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(duplicated)
}

fn rebind_tcp(stream: &mut TcpStream) -> io::Result<()> {
    let duplicated = duplicate_socket(stream.as_raw_fd())?;
    let replacement = TcpStream::from_std(duplicated.into())?;
    // Drop the old registration only after the replacement is fully registered.
    std::mem::drop(std::mem::replace(stream, replacement));
    Ok(())
}

fn rebind_unix(stream: &mut UnixStream) -> io::Result<()> {
    let duplicated = duplicate_socket(stream.as_raw_fd())?;
    let replacement = UnixStream::from_std(duplicated.into())?;
    std::mem::drop(std::mem::replace(stream, replacement));
    Ok(())
}

pub(crate) async fn create_unix_stream_inner(host: &str, port: u16) -> Result<StreamInner, Error> {
    let started = Instant::now();
    let stream = match UnixStream::connect(&format!("{host}/.s.PGSQL.{port}")).await {
        Ok(s) => s,
        Err(err) => {
            log::error!("Failed to connect to Unix socket {host}:{port}: {err}");
            return Err(connect_error_from_io(
                &format!("Unix socket {host}:{port}"),
                err,
            ));
        }
    };

    configure_unix_socket(&stream);

    crate::web::metrics::observe_backend_create_phase(
        "tcp_connect",
        started.elapsed().as_secs_f64(),
    );

    Ok(StreamInner::UnixSocket { stream })
}

pub(crate) async fn create_tcp_stream_inner(
    host: &str,
    port: u16,
    server_tls: &ServerTlsConfig,
    pool_name: &str,
) -> Result<StreamInner, Error> {
    let tcp_started = Instant::now();
    let mut stream = match TcpStream::connect(&format!("{host}:{port}")).await {
        Ok(stream) => stream,
        Err(err) => {
            log::error!("Failed to connect to TCP {host}:{port}: {err}");
            return Err(connect_error_from_io(&format!("{host}:{port}"), err));
        }
    };

    configure_tcp_socket(&stream);

    crate::web::metrics::observe_backend_create_phase(
        "tcp_connect",
        tcp_started.elapsed().as_secs_f64(),
    );

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
    let tls_started = Instant::now();
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
                    crate::web::metrics::SHOW_SERVER_TLS_HANDSHAKE_DURATION
                        .with_label_values(&[pool_name])
                        .observe(elapsed.as_secs_f64());
                    crate::web::metrics::observe_backend_create_phase(
                        "tls",
                        tls_started.elapsed().as_secs_f64(),
                    );
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
                    crate::web::metrics::SHOW_SERVER_TLS_HANDSHAKE_ERRORS
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
                crate::web::metrics::SHOW_SERVER_TLS_HANDSHAKE_ERRORS
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

fn connect_error_from_io(target: &str, err: io::Error) -> Error {
    let msg = format!("Could not connect to {target}: {err}");
    if is_fd_exhaustion_io(&err) {
        Error::ConnectResourceExhausted(msg)
    } else {
        Error::ConnectError(msg)
    }
}

#[cfg(unix)]
fn is_fd_exhaustion_io(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE))
}

#[cfg(not(unix))]
fn is_fd_exhaustion_io(_err: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn connect_error_from_io_keeps_fd_exhaustion_typed() {
        let err = io::Error::from_raw_os_error(libc::EMFILE);

        match connect_error_from_io("127.0.0.1:5432", err) {
            Error::ConnectResourceExhausted(msg) => {
                assert!(msg.contains("127.0.0.1:5432"), "unexpected msg: {msg}");
            }
            other => panic!("expected ConnectResourceExhausted, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn connect_error_from_io_keeps_regular_connect_errors_unreachable() {
        let err = io::Error::from_raw_os_error(libc::ECONNREFUSED);

        match connect_error_from_io("127.0.0.1:5432", err) {
            Error::ConnectError(msg) => {
                assert!(msg.contains("127.0.0.1:5432"), "unexpected msg: {msg}");
            }
            other => panic!("expected ConnectError, got {other:?}"),
        }
    }

    #[test]
    fn tcp_rebind_preserves_connection_and_buffered_bytes() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream as StdTcpStream};
        use tokio::io::{AsyncReadExt, AsyncWriteExt, BufStream};
        use tokio::runtime::Builder;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = StdTcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let (accepted, _) = listener.accept().unwrap();
        accepted.set_nonblocking(true).unwrap();

        let old_runtime = Builder::new_current_thread().enable_io().build().unwrap();
        let mut stream = old_runtime.block_on(async {
            let stream = TcpStream::from_std(accepted).unwrap();
            BufStream::new(StreamInner::TCPPlain { stream })
        });
        let mut byte = [0u8; 1];
        let err = stream.get_ref().try_read(&mut byte).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);

        client.write_all(b"buffered").unwrap();
        old_runtime.block_on(async {
            let mut first = [0; 4];
            stream.read_exact(&mut first).await.unwrap();
            assert_eq!(&first, b"buff");
        });

        let new_runtime = Builder::new_current_thread().enable_io().build().unwrap();
        new_runtime.block_on(async {
            stream.get_mut().rebind_to_current_worker().await.unwrap();
            let mut rest = [0; 4];
            stream.read_exact(&mut rest).await.unwrap();
            assert_eq!(&rest, b"ered");
            stream.write_all(b"pong").await.unwrap();
            stream.flush().await.unwrap();
        });
        let mut response = [0; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");

        client.write_all(b"new").unwrap();
        new_runtime.block_on(async {
            let mut data = [0; 3];
            stream.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"new");
        });
    }

    #[test]
    fn unix_rebind_preserves_connection_across_runtimes() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream as StdUnixStream;
        use tokio::io::AsyncReadExt;
        use tokio::runtime::Builder;

        let (mut client, accepted) = StdUnixStream::pair().unwrap();
        accepted.set_nonblocking(true).unwrap();
        let old_runtime = Builder::new_current_thread().enable_io().build().unwrap();
        let stream = old_runtime.block_on(async { UnixStream::from_std(accepted).unwrap() });
        let mut stream = StreamInner::UnixSocket { stream };
        let new_runtime = Builder::new_current_thread().enable_io().build().unwrap();

        new_runtime.block_on(async {
            stream.rebind_to_current_worker().await.unwrap();
            stream.write_all(b"pong").await.unwrap();
        });
        let mut response = [0; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");

        client.write_all(b"new").unwrap();
        new_runtime.block_on(async {
            let mut data = [0; 3];
            stream.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"new");
        });
    }

    #[test]
    fn tls_rebind_preserves_session_across_runtimes() {
        use std::io::{Read, Write};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::runtime::Builder;

        let rsa = openssl::rsa::Rsa::generate(2048).unwrap();
        let key = openssl::pkey::PKey::from_rsa(rsa).unwrap();
        let mut name = openssl::x509::X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut cert = openssl::x509::X509Builder::new().unwrap();
        cert.set_version(2).unwrap();
        let serial = openssl::bn::BigNum::from_u32(1)
            .unwrap()
            .to_asn1_integer()
            .unwrap();
        cert.set_serial_number(&serial).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&openssl::asn1::Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&openssl::asn1::Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        cert.sign(&key, openssl::hash::MessageDigest::sha256())
            .unwrap();
        let cert = cert.build();
        let identity = native_tls::Identity::from_pkcs8(
            &cert.to_pem().unwrap(),
            &key.private_key_to_pem_pkcs8().unwrap(),
        )
        .unwrap();
        let acceptor =
            tokio_native_tls::TlsAcceptor::from(native_tls::TlsAcceptor::new(identity).unwrap());
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        accepted.set_nonblocking(true).unwrap();
        let client_thread =
            std::thread::spawn(move || connector.connect("localhost", client).unwrap());

        let old_runtime = Builder::new_current_thread().enable_io().build().unwrap();
        let server = old_runtime.block_on(async {
            let server = TcpStream::from_std(accepted).unwrap();
            acceptor.accept(server).await.unwrap()
        });
        let mut client = client_thread.join().unwrap();

        let new_runtime = Builder::new_current_thread().enable_io().build().unwrap();
        let mut server = StreamInner::TCPTls { stream: server };
        new_runtime.block_on(async {
            server.rebind_to_current_worker().await.unwrap();
            server.write_all(b"pong").await.unwrap();
            server.flush().await.unwrap();
        });
        let mut response = [0; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");

        client.write_all(b"new").unwrap();
        client.flush().unwrap();
        new_runtime.block_on(async {
            let mut data = [0; 3];
            server.read_exact(&mut data).await.unwrap();
            assert_eq!(&data, b"new");
        });
    }
}
