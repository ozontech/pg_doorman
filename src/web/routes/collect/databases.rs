use crate::pool::get_all_pools;
use crate::web::routes::dto::{DatabaseDto, DatabasesDto};

use super::now_unix_ms;

pub(crate) fn collect_databases() -> DatabasesDto {
    let pools_map = get_all_pools();
    let mut databases: Vec<DatabaseDto> = pools_map
        .iter()
        .map(|(_identifier, pool)| {
            let address = pool.address();
            let settings = &pool.settings;
            DatabaseDto {
                name: address.name(),
                host: address.host.clone(),
                port: address.port,
                database: address.database.clone(),
                force_user: settings.user.username.clone(),
                pool_size: settings.user.pool_size,
                min_pool_size: settings.user.min_pool_size.unwrap_or(0),
                // See DatabaseDto::reserve_pool — mirrors SHOW DATABASES quirk.
                reserve_pool: 0,
                pool_mode: settings.pool_mode.to_string(),
                max_connections: settings.user.pool_size,
                current_connections: pool.pool_state().size as u32,
            }
        })
        .collect();

    // Deterministic order using the pool name composite key.
    databases.sort_by(|a, b| a.name.cmp(&b.name));

    DatabasesDto {
        ts: now_unix_ms(),
        databases,
    }
}
