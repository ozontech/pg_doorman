use crate::pool::get_all_pools;
use crate::web::routes::dto::{UserDto, UsersDto};

use super::now_unix_ms;

pub(crate) fn collect_users() -> UsersDto {
    let pools_map = get_all_pools();
    let mut users: Vec<UserDto> = pools_map
        .iter()
        .map(|(identifier, pool)| UserDto {
            name: identifier.user.clone(),
            pool_mode: pool.settings.pool_mode.to_string(),
        })
        .collect();

    users.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.pool_mode.cmp(&b.pool_mode))
    });

    UsersDto {
        ts: now_unix_ms(),
        users,
    }
}
