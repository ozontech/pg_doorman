# rust-native-tls (pg_doorman patch)

This is a patched fork of [rust-native-tls](https://docs.rs/native-tls) v0.2.14, maintained as part of the pg_doorman project. The upstream crate provides a cross-platform TLS abstraction; this patch extends the OpenSSL backend with features required by a PostgreSQL connection pooler.

## Why a patch instead of upstream contribution

The upstream `native-tls` crate intentionally provides a minimal, cross-platform API. The extensions below are Linux/OpenSSL-specific and would not fit the crate's design goals.

## Patch summary

### 1. Client certificate verification (`TlsClientCertificateVerification`)

PostgreSQL's `verify-full` TLS mode requires the server to request and verify client certificates. Upstream `native-tls` has no API for server-side client cert verification — the `TlsAcceptor` always uses `SslVerifyMode::NONE`.

This patch adds:
- `TlsClientCertificateVerification` enum (`DoNotRequestCertificate` / `RequireCertificate`)
- `TlsAcceptorBuilder::client_cert_verification()` setter
- `TlsAcceptorBuilder::client_cert_verification_ca_cert()` to specify the CA for client cert validation

These map to OpenSSL's `SSL_CTX_set_verify(SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT)` and `SSL_CTX_add_client_CA()`.

### 2. Kernel TLS (kTLS) offloading

After the TLS handshake completes in userspace, OpenSSL can push the negotiated symmetric encryption keys to the Linux kernel via `setsockopt(SOL_TLS)`. The kernel then handles record-layer encryption and decryption transparently, reducing context switches and enabling zero-copy `sendfile()` for TLS streams.

This patch adds:
- `TlsAcceptorBuilder::enable_ktls(bool)` — sets `SSL_OP_ENABLE_KTLS` (bit 3, `0x8`) on the SSL context
- `TlsStream::is_ktls_send_active()` — checks `BIO_CTRL_GET_KTLS_SEND` (73) on the write BIO
- `TlsStream::is_ktls_recv_active()` — checks `BIO_CTRL_GET_KTLS_RECV` (76) on the read BIO
- `TlsStream::negotiated_cipher()` — returns the name of the negotiated cipher suite (e.g. `TLS_AES_256_GCM_SHA384`), used for diagnostics when kTLS fails to activate
- `TlsStream::protocol_version()` — returns the negotiated TLS protocol version (e.g. `TLSv1.3`)

Requirements: OpenSSL 3.0+, Linux kernel with the `tls` module loaded (`modprobe tls`). If the kernel module is not loaded or the negotiated cipher is not supported by kTLS, OpenSSL silently falls back to userspace encryption — no error is raised.

kTLS detection methods use `openssl-sys` FFI directly (`SSL_get_wbio`, `SSL_get_rbio`, `BIO_ctrl`) because the `openssl` crate (0.10.x) does not expose kTLS APIs.

### 3. TLS mode support

The `supported_protocols()` function is used by both the connector and acceptor to enforce minimum/maximum TLS protocol versions. pg_doorman sets minimum TLS 1.2 via the `TlsAcceptorBuilder::min_protocol_version()` API (already present in upstream).

## Original README

[Documentation](https://docs.rs/native-tls)

An abstraction over platform-specific TLS implementations.

Specifically, this crate uses SChannel on Windows (via the [`schannel`] crate),
Secure Transport on macOS (via the [`security-framework`] crate), and OpenSSL (via
the [`openssl`] crate) on all other platforms.

[`schannel`]: https://crates.io/crates/schannel
[`security-framework`]: https://crates.io/crates/security-framework
[`openssl`]: https://crates.io/crates/openssl

## Installation

```toml
# Cargo.toml
[dependencies]
native-tls = "0.2"
```

## Usage

An example client looks like:

```rust,ignore
extern crate native_tls;

use native_tls::TlsConnector;
use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    let connector = TlsConnector::new().unwrap();

    let stream = TcpStream::connect("google.com:443").unwrap();
    let mut stream = connector.connect("google.com", stream).unwrap();

    stream.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
    let mut res = vec![];
    stream.read_to_end(&mut res).unwrap();
    println!("{}", String::from_utf8_lossy(&res));
}
```

To accept connections as a server from remote clients:

```rust,ignore
extern crate native_tls;

use native_tls::{Identity, TlsAcceptor, TlsStream};
use std::fs::File;
use std::io::{Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

fn main() {
    let mut file = File::open("identity.pfx").unwrap();
    let mut identity = vec![];
    file.read_to_end(&mut identity).unwrap();
    let identity = Identity::from_pkcs12(&identity, "hunter2").unwrap();

    let acceptor = TlsAcceptor::new(identity).unwrap();
    let acceptor = Arc::new(acceptor);

    let listener = TcpListener::bind("0.0.0.0:8443").unwrap();

    fn handle_client(stream: TlsStream<TcpStream>) {
        // ...
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let acceptor = acceptor.clone();
                thread::spawn(move || {
                    let stream = acceptor.accept(stream).unwrap();
                    handle_client(stream);
                });
            }
            Err(e) => { /* connection failed */ }
        }
    }
}
```

# License

`rust-native-tls` is primarily distributed under the terms of both the MIT
license and the Apache License (Version 2.0), with portions covered by various
BSD-like licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.
