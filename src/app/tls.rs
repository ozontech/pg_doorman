use log::{error, info, warn};
use std::path::Path;

use crate::config::{Config, KtlsMode};
use crate::tls::build_acceptor;
use crate::utils::rate_limit::RateLimiter;

#[derive(Clone)]
pub struct TlsState {
    pub rate_limiter: Option<RateLimiter>,
    pub acceptor: Option<tokio_native_tls::TlsAcceptor>,
    pub ktls_mode: KtlsMode,
}

/// Check if the system supports kTLS:
/// - Linux kernel module `tls` is loaded (`/sys/module/tls/` exists)
/// - OpenSSL version is 3.0+ (kTLS support added in 3.0)
///
/// Returns `(kernel_ok, openssl_ok)`.
fn check_ktls_system_support() -> (bool, bool) {
    let kernel_ok = Path::new("/sys/module/tls").exists();
    // OpenSSL version number: 0x30000000 = 3.0.0
    let openssl_version = openssl::version::number();
    let openssl_ok = openssl_version >= 0x3000_0000;
    (kernel_ok, openssl_ok)
}

pub fn init_tls(config: &Config) -> TlsState {
    let rate_limiter: Option<RateLimiter> = if config.general.tls_rate_limit_per_second > 0 {
        info!(
            "Building rate limit: {} per second",
            config.general.tls_rate_limit_per_second
        );
        let rate = std::cmp::max(1, config.general.tls_rate_limit_per_second / 100);
        Some(RateLimiter::new(rate, 10))
    } else {
        None
    };

    let ktls_mode = config.general.ktls;
    let enable_ktls = ktls_mode != KtlsMode::Off;

    // Validate system kTLS support at startup
    if enable_ktls {
        let (kernel_ok, openssl_ok) = check_ktls_system_support();
        let openssl_ver = openssl::version::version();

        if !kernel_ok {
            warn!("kTLS mode: try — kernel module 'tls' is not loaded, kTLS will not activate (modprobe tls)");
        }
        if !openssl_ok {
            warn!(
                "kTLS mode: try — OpenSSL 3.0+ required for kTLS, current: {}",
                openssl_ver
            );
        }
        if kernel_ok && openssl_ok {
            info!(
                "kTLS mode: try (kernel module: loaded, OpenSSL: {})",
                openssl_ver
            );
        }
    }

    let acceptor: Option<tokio_native_tls::TlsAcceptor> =
        if config.general.tls_certificate.is_some() {
            match build_acceptor(
                Path::new(&config.general.tls_certificate.clone().unwrap()),
                Path::new(&config.general.tls_private_key.clone().unwrap()),
                config.general.tls_ca_cert.clone(),
                config.general.tls_mode.clone(),
                enable_ktls,
            ) {
                Ok(acceptor) => Some(acceptor),
                Err(err) => {
                    error!("Failed to build TLS acceptor: {err}");
                    std::process::exit(exitcode::CONFIG);
                }
            }
        } else {
            None
        };

    TlsState {
        rate_limiter,
        acceptor,
        ktls_mode,
    }
}
