use crate::stats::{
    CANCEL_CONNECTION_COUNTER, PLAIN_CONNECTION_COUNTER, TLS_CONNECTION_COUNTER,
    TOTAL_CONNECTION_COUNTER,
};
use crate::web::routes::dto::ConnectionsDto;

use super::{cnt, now_unix_ms};

pub(crate) fn collect_connections() -> ConnectionsDto {
    connections_from_raw(
        cnt(&TOTAL_CONNECTION_COUNTER),
        cnt(&TLS_CONNECTION_COUNTER),
        cnt(&PLAIN_CONNECTION_COUNTER),
        cnt(&CANCEL_CONNECTION_COUNTER),
    )
}

/// Builds a `ConnectionsDto` from raw counter values. Pure function — exists
/// so the `errors = total - tls - plain - cancel` derivation is exercised by
/// unit tests without touching the global atomics.
fn connections_from_raw(total: u64, tls: u64, plain: u64, cancel: u64) -> ConnectionsDto {
    ConnectionsDto {
        ts: now_unix_ms(),
        total,
        tls,
        plain,
        cancel,
        // `errors` mirrors `SHOW CONNECTIONS`: it is whatever is left after
        // subtracting the categorised counters from the total. May be zero or
        // positive in normal operation.
        errors: total
            .saturating_sub(tls)
            .saturating_sub(plain)
            .saturating_sub(cancel),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn connections_errors_derive_from_total_minus_categorised() {
        let dto = super::connections_from_raw(100, 60, 30, 5);
        assert_eq!(dto.total, 100);
        assert_eq!(dto.tls, 60);
        assert_eq!(dto.plain, 30);
        assert_eq!(dto.cancel, 5);
        assert_eq!(dto.errors, 5);
    }

    #[test]
    fn connections_errors_zero_when_categories_cover_total() {
        let dto = super::connections_from_raw(50, 30, 15, 5);
        assert_eq!(dto.errors, 0);
    }

    #[test]
    fn connections_errors_saturate_when_categories_exceed_total() {
        // Race: categorised counters momentarily ahead of total.
        // Without saturating_sub this would underflow into u64::MAX.
        let dto = super::connections_from_raw(10, 8, 5, 0);
        assert_eq!(dto.errors, 0);
    }
}
