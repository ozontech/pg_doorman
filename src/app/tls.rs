use log::{error, info};
use std::path::Path;

use crate::config::Config;
use crate::rate_limit::RateLimiter;
use crate::tls::build_acceptor;

#[derive(Clone)]
pub struct TlsState {
    pub rate_limiter: Option<RateLimiter>,
    pub acceptor: Option<tokio_native_tls::TlsAcceptor>,
}

pub fn init_tls(config: &Config) -> TlsState {
    // Не обновляется по HUP (как и в исходном `main`).
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

    // Не обновляется по HUP (как и в исходном `main`).
    let acceptor: Option<tokio_native_tls::TlsAcceptor> = if config.general.tls_certificate.is_some() {
        match build_acceptor(
            Path::new(&config.general.tls_certificate.clone().unwrap()),
            Path::new(&config.general.tls_private_key.clone().unwrap()),
            config.general.tls_ca_cert.clone(),
            config.general.tls_mode.clone(),
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
    }
}
