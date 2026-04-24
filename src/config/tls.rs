use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

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

impl std::str::FromStr for ServerTlsMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "disable" => Ok(Self::Disable),
            "allow" => Ok(Self::Allow),
            "prefer" => Ok(Self::Prefer),
            "require" => Ok(Self::Require),
            "verify-ca" => Ok(Self::VerifyCa),
            "verify-full" => Ok(Self::VerifyFull),
            _ => Err(Error::BadConfig(format!("invalid server_tls_mode: {s}"))),
        }
    }
}

impl ServerTlsMode {
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

/// Load all certificates from a PEM file (supports bundles with
/// intermediate CA chains — multiple PEM blocks in one file).
fn load_certificates(path: &Path) -> Result<Vec<Certificate>, Error> {
    let cert_data = read_file(path).map_err(|err| {
        Error::BadConfig(format!(
            "Failed to read certificate file {}: {}",
            path.display(),
            err
        ))
    })?;

    let pem_str = std::str::from_utf8(&cert_data).map_err(|err| {
        Error::BadConfig(format!(
            "Certificate file {} is not valid UTF-8: {}",
            path.display(),
            err
        ))
    })?;

    let mut certs = Vec::new();
    let mut start = 0;
    let begin_marker = "-----BEGIN CERTIFICATE-----";
    let end_marker = "-----END CERTIFICATE-----";

    while let Some(begin) = pem_str[start..].find(begin_marker) {
        let abs_begin = start + begin;
        let after_begin = abs_begin + begin_marker.len();
        match pem_str[after_begin..].find(end_marker) {
            Some(end_offset) => {
                let abs_end = after_begin + end_offset + end_marker.len();
                let pem_block = &pem_str[abs_begin..abs_end];
                let cert = Certificate::from_pem(pem_block.as_bytes()).map_err(|err| {
                    Error::BadConfig(format!(
                        "Failed to parse certificate #{} in {}: {}",
                        certs.len() + 1,
                        path.display(),
                        err
                    ))
                })?;
                certs.push(cert);
                start = abs_end;
            }
            None => {
                return Err(Error::BadConfig(format!(
                    "Unterminated PEM block in {}: found BEGIN without END",
                    path.display(),
                )));
            }
        }
    }

    if certs.is_empty() {
        return Err(Error::BadConfig(format!(
            "No certificates found in {}",
            path.display(),
        )));
    }

    Ok(certs)
}

/// Resolved TLS configuration for server-facing connections.
#[derive(Debug)]
pub struct ServerTlsConfig {
    pub mode: ServerTlsMode,
    pub connector: Option<tokio_native_tls::TlsConnector>,
    /// SHA-256 hash of certificate file contents (ca + client cert + client key).
    /// Used to detect cert changes on SIGHUP reload without comparing opaque
    /// TlsConnector objects.
    pub cert_hash: Option<[u8; 32]>,
}

/// Manual impl: `connector` is opaque (no PartialEq), so equality is
/// determined by `mode` + `cert_hash`. Update this if new config fields
/// are added to `ServerTlsConfig`.
impl PartialEq for ServerTlsConfig {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode && self.cert_hash == other.cert_hash
    }
}

impl Eq for ServerTlsConfig {}

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
                cert_hash: None,
            });
        }

        if mode.requires_ca() && ca_cert.is_none() {
            return Err(Error::BadConfig(format!(
                "server_tls_mode '{}' requires server_tls_ca_cert to be set",
                mode
            )));
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
                    for ca in load_certificates(ca_path)? {
                        builder.add_root_certificate(ca);
                    }
                }
                builder.danger_accept_invalid_hostnames(true);
            }
            ServerTlsMode::VerifyFull => {
                if let Some(ca_path) = ca_cert {
                    for ca in load_certificates(ca_path)? {
                        builder.add_root_certificate(ca);
                    }
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

        let cert_hash = {
            let mut hasher = Sha256::new();
            if let Some(ca_path) = ca_cert {
                if let Ok(data) = read_file(ca_path) {
                    hasher.update(&data);
                }
            }
            if let Some(cert_path) = client_cert {
                if let Ok(data) = read_file(cert_path) {
                    hasher.update(&data);
                }
            }
            if let Some(key_path) = client_key {
                if let Ok(data) = read_file(key_path) {
                    hasher.update(&data);
                }
            }
            Some(hasher.finalize().into())
        };

        Ok(ServerTlsConfig {
            mode,
            connector: Some(connector),
            cert_hash,
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

    // Load CA certificate if provided (only the first certificate is used for client verification)
    let ca = match ca_path {
        Some(path) => {
            let path = path.as_ref();
            let mut certs = load_certificates(path)?;
            Some(certs.remove(0))
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
    fn test_server_tls_mode_from_str() {
        assert_eq!(
            "disable".parse::<ServerTlsMode>().unwrap(),
            ServerTlsMode::Disable
        );
        assert_eq!(
            "allow".parse::<ServerTlsMode>().unwrap(),
            ServerTlsMode::Allow
        );
        assert_eq!(
            "prefer".parse::<ServerTlsMode>().unwrap(),
            ServerTlsMode::Prefer
        );
        assert_eq!(
            "require".parse::<ServerTlsMode>().unwrap(),
            ServerTlsMode::Require
        );
        assert_eq!(
            "verify-ca".parse::<ServerTlsMode>().unwrap(),
            ServerTlsMode::VerifyCa
        );
        assert_eq!(
            "verify-full".parse::<ServerTlsMode>().unwrap(),
            ServerTlsMode::VerifyFull
        );
        assert!("invalid".parse::<ServerTlsMode>().is_err());
        assert!("".parse::<ServerTlsMode>().is_err());
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
    fn test_server_tls_config_verify_ca_without_ca_cert_is_error() {
        let err = ServerTlsConfig::new(ServerTlsMode::VerifyCa, None, None, None).unwrap_err();
        assert!(
            err.to_string().contains("server_tls_ca_cert"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_server_tls_config_verify_full_without_ca_cert_is_error() {
        let err = ServerTlsConfig::new(ServerTlsMode::VerifyFull, None, None, None).unwrap_err();
        assert!(
            err.to_string().contains("server_tls_ca_cert"),
            "unexpected error: {err}"
        );
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
    fn test_load_certificates() {
        // These paths are relative to the project root
        let cert_path = PathBuf::from("tests/data/ssl/server.crt");

        if cert_path.exists() {
            let result = load_certificates(&cert_path);
            assert!(
                result.is_ok(),
                "Failed to load certificates: {:?}",
                result.err()
            );
            assert!(
                !result.unwrap().is_empty(),
                "Expected at least one certificate"
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
