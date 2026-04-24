use std::io::{self, Read};
use std::path::Path;

use crate::errors::Error;
use native_tls::TlsClientCertificateVerification::{DoNotRequestCertificate, RequireCertificate};
use native_tls::{Certificate, Identity, Protocol, TlsClientCertificateVerification};

fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut content = Vec::new();
    let mut file = std::fs::File::open(path)?;
    file.read_to_end(&mut content)?;
    Ok(content)
}

pub fn load_identity(cert: &Path, key: &Path) -> io::Result<Identity> {
    let cert_body = read_file(cert)?;
    let key_body = read_file(key)?;

    Identity::from_pkcs8(&cert_body, &key_body).map_err(|err| io::Error::other(err.to_string()))
}

/// TLS mode for server-facing (backend) connections.
/// Ordered from least to most secure, matching libpq sslmode semantics.
#[derive(Default, PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone)]
pub enum ServerTlsMode {
    /// Do not use TLS
    Disable,
    /// Try plain first; if server rejects, retry with TLS on a new TCP socket.
    /// Matches libpq sslmode=allow: "first try a non-SSL connection;
    /// if that fails, try an SSL connection."
    #[default]
    Allow,
    /// Send SSLRequest first; if server says 'N', fall back to plain
    Prefer,
    /// Require TLS, fail if server doesn't support it
    Require,
    /// Require TLS and verify server certificate against CA
    VerifyCa,
    /// Require TLS, verify CA, and verify hostname matches certificate
    VerifyFull,
}

impl std::fmt::Display for ServerTlsMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerTlsMode::Disable => write!(f, "disable"),
            ServerTlsMode::Allow => write!(f, "allow"),
            ServerTlsMode::Prefer => write!(f, "prefer"),
            ServerTlsMode::Require => write!(f, "require"),
            ServerTlsMode::VerifyCa => write!(f, "verify-ca"),
            ServerTlsMode::VerifyFull => write!(f, "verify-full"),
        }
    }
}

impl ServerTlsMode {
    pub fn from_string(s: &str) -> Result<Self, Error> {
        match s {
            "disable" => Ok(ServerTlsMode::Disable),
            "allow" => Ok(ServerTlsMode::Allow),
            "prefer" => Ok(ServerTlsMode::Prefer),
            "require" => Ok(ServerTlsMode::Require),
            "verify-ca" => Ok(ServerTlsMode::VerifyCa),
            "verify-full" => Ok(ServerTlsMode::VerifyFull),
            _ => Err(Error::BadConfig(format!("Invalid server_tls_mode: {s}"))),
        }
    }

    /// Whether this mode requires a CA certificate to be configured.
    pub fn requires_ca(&self) -> bool {
        matches!(self, ServerTlsMode::VerifyCa | ServerTlsMode::VerifyFull)
    }

    /// Whether this mode sends an SSLRequest on the first connection attempt.
    /// `Allow` skips SSLRequest initially (tries plain first, retries with TLS on failure).
    pub fn sends_ssl_request(&self) -> bool {
        !matches!(self, ServerTlsMode::Disable | ServerTlsMode::Allow)
    }

    /// Whether this mode requires the server to support TLS.
    pub fn requires_tls(&self) -> bool {
        matches!(
            self,
            ServerTlsMode::Require | ServerTlsMode::VerifyCa | ServerTlsMode::VerifyFull
        )
    }

    /// Whether this mode retries with TLS after a plain connection failure.
    pub fn retries_with_tls(&self) -> bool {
        matches!(self, ServerTlsMode::Allow)
    }
}

/// TLS mode options for connections
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone)]
pub enum TLSMode {
    /// Allow but don't require TLS
    Allow,
    /// Disable TLS
    Disable,
    /// Require TLS but don't verify certificates
    Require,
    /// Require TLS and verify certificates
    VerifyFull,
}

impl std::fmt::Display for TLSMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TLSMode::Allow => write!(f, "allow"),
            TLSMode::Disable => write!(f, "disable"),
            TLSMode::Require => write!(f, "require"),
            TLSMode::VerifyFull => write!(f, "verify-full"),
        }
    }
}

impl TLSMode {
    /// Convert a string to a TLSMode
    pub fn from_string(s: &str) -> Result<Self, Error> {
        match s {
            "allow" => Ok(TLSMode::Allow),
            "disable" => Ok(TLSMode::Disable),
            "require" => Ok(TLSMode::Require),
            "verify-full" => Ok(TLSMode::VerifyFull),
            _ => Err(Error::BadConfig(format!("Invalid tls_mode: {s}"))),
        }
    }
}

/// Convert TLSMode to native_tls TlsClientCertificateVerification
#[allow(dead_code)]
fn tls_mode_to_verification(mode: &str) -> Result<TlsClientCertificateVerification, Error> {
    let tls_mode = TLSMode::from_string(mode)?;
    match tls_mode {
        TLSMode::Require | TLSMode::Allow => Ok(DoNotRequestCertificate),
        TLSMode::VerifyFull => Ok(RequireCertificate),
        TLSMode::Disable => Err(Error::BadConfig(
            "TLS mode 'disable' cannot be used when TLS is enabled".to_string(),
        )),
    }
}

/// Load a certificate from a PEM file
fn load_certificate(path: &Path) -> Result<Certificate, Error> {
    let cert_data = read_file(path).map_err(|err| {
        Error::BadConfig(format!(
            "Failed to read certificate file {}: {}",
            path.display(),
            err
        ))
    })?;

    Certificate::from_pem(&cert_data).map_err(|err| {
        Error::BadConfig(format!(
            "Failed to parse certificate {}: {}",
            path.display(),
            err
        ))
    })
}

/// Resolved TLS configuration for server-facing connections.
#[derive(Debug)]
pub struct ServerTlsConfig {
    pub mode: ServerTlsMode,
    pub connector: Option<tokio_native_tls::TlsConnector>,
}

impl ServerTlsConfig {
    pub fn new(
        mode: ServerTlsMode,
        ca_cert: Option<&Path>,
        client_cert: Option<&Path>,
        client_key: Option<&Path>,
    ) -> Result<Self, Error> {
        if mode == ServerTlsMode::Disable {
            return Ok(ServerTlsConfig {
                mode,
                connector: None,
            });
        }

        let mut builder = native_tls::TlsConnector::builder();
        builder.min_protocol_version(Some(Protocol::Tlsv12));

        match mode {
            ServerTlsMode::Allow | ServerTlsMode::Prefer | ServerTlsMode::Require => {
                builder.danger_accept_invalid_certs(true);
                builder.danger_accept_invalid_hostnames(true);
            }
            ServerTlsMode::VerifyCa => {
                if let Some(ca_path) = ca_cert {
                    let ca = load_certificate(ca_path)?;
                    builder.add_root_certificate(ca);
                }
                builder.danger_accept_invalid_hostnames(true);
            }
            ServerTlsMode::VerifyFull => {
                if let Some(ca_path) = ca_cert {
                    let ca = load_certificate(ca_path)?;
                    builder.add_root_certificate(ca);
                }
            }
            ServerTlsMode::Disable => unreachable!(),
        }

        // mTLS: present client certificate to server
        if let (Some(cert_path), Some(key_path)) = (client_cert, client_key) {
            let identity = load_identity(cert_path, key_path).map_err(|err| {
                Error::BadConfig(format!(
                    "Failed to load server TLS client identity from {} and {}: {}",
                    cert_path.display(),
                    key_path.display(),
                    err
                ))
            })?;
            builder.identity(identity);
        }

        let connector = builder
            .build()
            .map(tokio_native_tls::TlsConnector::from)
            .map_err(|err| {
                Error::BadConfig(format!("Failed to create server TLS connector: {err}"))
            })?;

        Ok(ServerTlsConfig {
            mode,
            connector: Some(connector),
        })
    }
}

/// Build a TLS acceptor from certificate, key, and optional CA certificate
#[allow(unused_variables)]
pub fn build_acceptor(
    cert: &Path,
    key: &Path,
    ca_path: Option<impl AsRef<Path>>,
    mode: Option<String>,
) -> Result<tokio_native_tls::TlsAcceptor, Error> {
    // Load identity from certificate and key
    let identity = load_identity(cert, key).map_err(|err| {
        Error::BadConfig(format!(
            "Failed to load TLS identity from cert {} and key {}: {}",
            cert.display(),
            key.display(),
            err
        ))
    })?;

    // Load CA certificate if provided
    let ca = match ca_path {
        Some(path) => {
            let path = path.as_ref();
            Some(load_certificate(path)?)
        }
        None => None,
    };

    // Build TLS acceptor
    let mut builder = native_tls::TlsAcceptor::builder(identity);

    // Set protocol versions
    builder.min_protocol_version(Some(Protocol::Tlsv12)); // Upgraded from Tlsv10 for better security
    builder.max_protocol_version(None);

    // Configure client certificate verification
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "ios")))]
    if let Some(ca_cert) = ca {
        builder.client_cert_verification_ca_cert(Some(ca_cert));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "ios")))]
    if let Some(mode_str) = mode {
        let verification = tls_mode_to_verification(mode_str.as_str())?;
        builder.client_cert_verification(verification);
    }

    // Build and convert to tokio acceptor
    builder
        .build()
        .map(tokio_native_tls::TlsAcceptor::from)
        .map_err(|err| Error::BadConfig(format!("Failed to create TLS acceptor: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_server_tls_mode_from_string() {
        assert_eq!(
            ServerTlsMode::from_string("disable").unwrap(),
            ServerTlsMode::Disable
        );
        assert_eq!(
            ServerTlsMode::from_string("allow").unwrap(),
            ServerTlsMode::Allow
        );
        assert_eq!(
            ServerTlsMode::from_string("prefer").unwrap(),
            ServerTlsMode::Prefer
        );
        assert_eq!(
            ServerTlsMode::from_string("require").unwrap(),
            ServerTlsMode::Require
        );
        assert_eq!(
            ServerTlsMode::from_string("verify-ca").unwrap(),
            ServerTlsMode::VerifyCa
        );
        assert_eq!(
            ServerTlsMode::from_string("verify-full").unwrap(),
            ServerTlsMode::VerifyFull
        );
        assert!(ServerTlsMode::from_string("invalid").is_err());
        assert!(ServerTlsMode::from_string("").is_err());
    }

    #[test]
    fn test_server_tls_mode_display() {
        assert_eq!(ServerTlsMode::Disable.to_string(), "disable");
        assert_eq!(ServerTlsMode::Allow.to_string(), "allow");
        assert_eq!(ServerTlsMode::Prefer.to_string(), "prefer");
        assert_eq!(ServerTlsMode::Require.to_string(), "require");
        assert_eq!(ServerTlsMode::VerifyCa.to_string(), "verify-ca");
        assert_eq!(ServerTlsMode::VerifyFull.to_string(), "verify-full");
    }

    #[test]
    fn test_server_tls_mode_requires_ca() {
        assert!(!ServerTlsMode::Disable.requires_ca());
        assert!(!ServerTlsMode::Allow.requires_ca());
        assert!(!ServerTlsMode::Prefer.requires_ca());
        assert!(!ServerTlsMode::Require.requires_ca());
        assert!(ServerTlsMode::VerifyCa.requires_ca());
        assert!(ServerTlsMode::VerifyFull.requires_ca());
    }

    #[test]
    fn test_tls_mode_from_string() {
        assert_eq!(TLSMode::from_string("allow").unwrap(), TLSMode::Allow);
        assert_eq!(TLSMode::from_string("disable").unwrap(), TLSMode::Disable);
        assert_eq!(TLSMode::from_string("require").unwrap(), TLSMode::Require);
        assert_eq!(
            TLSMode::from_string("verify-full").unwrap(),
            TLSMode::VerifyFull
        );

        // Test invalid mode
        assert!(TLSMode::from_string("invalid").is_err());
    }

    #[test]
    fn test_tls_mode_to_string() {
        assert_eq!(TLSMode::Allow.to_string(), "allow");
        assert_eq!(TLSMode::Disable.to_string(), "disable");
        assert_eq!(TLSMode::Require.to_string(), "require");
        assert_eq!(TLSMode::VerifyFull.to_string(), "verify-full");
    }

    #[test]
    fn test_tls_mode_to_verification() {
        // Valid modes
        assert!(matches!(
            tls_mode_to_verification("allow").unwrap(),
            TlsClientCertificateVerification::DoNotRequestCertificate
        ));
        assert!(matches!(
            tls_mode_to_verification("require").unwrap(),
            TlsClientCertificateVerification::DoNotRequestCertificate
        ));
        assert!(matches!(
            tls_mode_to_verification("verify-full").unwrap(),
            TlsClientCertificateVerification::RequireCertificate
        ));

        // Invalid mode
        assert!(tls_mode_to_verification("disable").is_err());
        assert!(tls_mode_to_verification("invalid").is_err());
    }

    #[test]
    fn test_server_tls_config_disable() {
        let config = ServerTlsConfig::new(ServerTlsMode::Disable, None, None, None).unwrap();
        assert_eq!(config.mode, ServerTlsMode::Disable);
        assert!(config.connector.is_none());
    }

    #[test]
    fn test_server_tls_config_prefer_no_certs() {
        let config = ServerTlsConfig::new(ServerTlsMode::Prefer, None, None, None).unwrap();
        assert_eq!(config.mode, ServerTlsMode::Prefer);
        assert!(config.connector.is_some());
    }

    #[test]
    fn test_server_tls_config_require_no_certs() {
        let config = ServerTlsConfig::new(ServerTlsMode::Require, None, None, None).unwrap();
        assert_eq!(config.mode, ServerTlsMode::Require);
        assert!(config.connector.is_some());
    }

    #[test]
    fn test_server_tls_config_verify_ca_with_cert() {
        let ca_path = PathBuf::from("tests/data/ssl/root.crt");
        if !ca_path.exists() {
            return; // skip if test certs not available
        }
        let config =
            ServerTlsConfig::new(ServerTlsMode::VerifyCa, Some(&ca_path), None, None).unwrap();
        assert_eq!(config.mode, ServerTlsMode::VerifyCa);
        assert!(config.connector.is_some());
    }

    #[test]
    fn test_read_file_nonexistent() {
        let result = read_file(PathBuf::from("/nonexistent/file"));
        assert!(result.is_err());
    }

    // Integration tests using actual certificate files
    #[test]
    fn test_load_certificate() {
        // These paths are relative to the project root
        let cert_path = PathBuf::from("tests/data/ssl/server.crt");

        if cert_path.exists() {
            let result = load_certificate(&cert_path);
            assert!(
                result.is_ok(),
                "Failed to load certificate: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn test_load_identity() {
        // These paths are relative to the project root
        let cert_path = PathBuf::from("tests/data/ssl/server.crt");
        let key_path = PathBuf::from("tests/data/ssl/server.key");

        if cert_path.exists() && key_path.exists() {
            let result = load_identity(&cert_path, &key_path);
            assert!(
                result.is_ok(),
                "Failed to load identity: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn test_build_acceptor() {
        // These paths are relative to the project root
        let cert_path = PathBuf::from("tests/data/ssl/server.crt");
        let key_path = PathBuf::from("tests/data/ssl/server.key");
        let ca_path = PathBuf::from("tests/data/ssl/root.crt");

        if cert_path.exists() && key_path.exists() && ca_path.exists() {
            // Test with CA and mode
            let result = build_acceptor(
                &cert_path,
                &key_path,
                Some(&ca_path),
                Some("require".to_string()),
            );
            assert!(
                result.is_ok(),
                "Failed to build acceptor with CA and mode: {:?}",
                result.err()
            );

            // Test without CA
            let result = build_acceptor(
                &cert_path,
                &key_path,
                None::<&Path>,
                Some("require".to_string()),
            );
            assert!(
                result.is_ok(),
                "Failed to build acceptor without CA and mode: {:?}",
                result.err()
            );

            // Test without mode
            let result = build_acceptor(&cert_path, &key_path, Some(&ca_path), None);
            assert!(
                result.is_ok(),
                "Failed to build acceptor without mode: {:?}",
                result.err()
            );
        }
    }
}
