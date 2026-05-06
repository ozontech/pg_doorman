use crate::app::log_level;
use crate::web::routes::dto::LogLevelDto;

use super::now_unix_ms;

pub(crate) fn collect_log_level() -> LogLevelDto {
    LogLevelDto {
        ts: now_unix_ms(),
        log_level: log_level::get_log_level(),
    }
}
