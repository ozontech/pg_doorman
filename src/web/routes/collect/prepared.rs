use crate::pool::get_all_pools;
use crate::web::routes::dto::{PreparedDto, PreparedRowDto, PreparedTextDto};

use super::now_unix_ms;

pub(crate) fn collect_prepared() -> PreparedDto {
    let mut prepared: Vec<PreparedRowDto> = Vec::new();
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        for (hash, parse, count_used, kind, hits, misses) in cache.get_entries() {
            prepared.push(PreparedRowDto {
                pool: identifier.to_string(),
                hash: hash.to_string(),
                name: parse.name.clone(),
                count_used,
                hits,
                misses,
                kind: kind.as_str().to_string(),
            });
        }
    }

    // Stable order: pool first, then hash, for deterministic UI display.
    prepared.sort_by(|a, b| {
        (a.pool.as_str(), a.hash.as_str()).cmp(&(b.pool.as_str(), b.hash.as_str()))
    });

    PreparedDto {
        ts: now_unix_ms(),
        prepared,
    }
}

pub(crate) fn collect_prepared_text(hash: u64) -> Option<PreparedTextDto> {
    for (identifier, pool) in get_all_pools().iter() {
        let Some(cache) = pool.prepared_statement_cache.as_ref() else {
            continue;
        };
        if let Some((parse, kind)) = cache.lookup_by_hash(hash) {
            return Some(PreparedTextDto {
                ts: now_unix_ms(),
                hash: format!("{:#x}", hash),
                pool: identifier.to_string(),
                name: parse.name.clone(),
                query: parse.query().to_string(),
                kind: kind.as_str().to_string(),
            });
        }
    }
    None
}
