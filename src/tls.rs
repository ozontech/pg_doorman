// Stream wrapper.
use std::io::Read;
use std::path::Path;

use native_tls::Identity;
use tokio_native_tls::native_tls;

// TLS

pub fn load_identity(cert: &Path, key: &Path) -> std::io::Result<Identity> {
    let mut cert_body = Vec::new();
    let mut fd_cert = std::fs::File::open(cert)?;
    fd_cert.read_to_end(&mut cert_body)?;
    let mut key_body = Vec::new();
    let mut fd_key = std::fs::File::open(key)?;
    fd_key.read_to_end(&mut key_body)?;
    match Identity::from_pkcs8(&cert_body, &key_body) {
        Ok(identity) => Ok(identity),
        Err(err) => Err(std::io::Error::other(err.to_string())),
    }
}
