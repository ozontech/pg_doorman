use crate::web::routes::dto::VersionDto;

use super::now_unix_ms;

pub(crate) fn collect_version() -> VersionDto {
    VersionDto {
        version: env!("CARGO_PKG_VERSION"),
        git_commit: option_env!("PG_DOORMAN_GIT_COMMIT").unwrap_or("unknown"),
        build_date: option_env!("PG_DOORMAN_BUILD_DATE").unwrap_or("unknown"),
        ts: now_unix_ms(),
    }
}
