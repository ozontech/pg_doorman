// TLS functionality for secure connections
use std::io::{self, Read};
use std::path::Path;

use crate::errors::Error;
use native_tls::TlsClientCertificateVerification::{DoNotRequestCertificate, RequireCertificate};
use native_tls::{Certificate, Identity, Protocol, TlsClientCertificateVerification};

/// Helper function to read a file into a byte vector
fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut content = Vec::new();
    let mut file = std::fs::File::open(path)?;
    file.read_to_end(&mut content)?;
    Ok(content)
}

/// Load identity from certificate and key files
pub fn load_identity(cert: &Path, key: &Path) -> io::Result<Identity> {
    let cert_body = read_file(cert)?;
    let key_body = read_file(key)?;

    Identity::from_pkcs8(&cert_body, &key_body).map_err(|err| io::Error::other(err.to_string()))
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
