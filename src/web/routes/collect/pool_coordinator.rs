use crate::pool::COORDINATORS;
use crate::web::routes::dto::{PoolCoordinatorDto, PoolCoordinatorRowDto};

use super::now_unix_ms;

pub(crate) fn collect_pool_coordinator() -> PoolCoordinatorDto {
    let coordinators = COORDINATORS.load();
    let mut databases: Vec<PoolCoordinatorRowDto> = coordinators
        .iter()
        .map(|(db, coordinator)| {
            let stats = coordinator.stats();
            let config = coordinator.config();
            PoolCoordinatorRowDto {
                database: db.clone(),
                max_db_conn: config.max_db_connections as u64,
                current: stats.total_connections as u64,
                reserve_size: config.reserve_pool_size as u64,
                reserve_used: stats.reserve_in_use as u64,
                evictions: stats.evictions_total,
                reserve_acq: stats.reserve_acquisitions_total,
                exhaustions: stats.exhaustions_total,
            }
        })
        .collect();

    databases.sort_by(|a, b| a.database.cmp(&b.database));

    PoolCoordinatorDto {
        ts: now_unix_ms(),
        databases,
    }
}
