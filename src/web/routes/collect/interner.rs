use crate::server::{anon_snapshot, named_snapshot, now_monotonic_ms};
use crate::web::routes::dto::{InternerDto, InternerKindDto, InternerTopDto, InternerTopRowDto};

use super::{clamp_top_n, now_unix_ms};

pub(crate) fn collect_interner() -> InternerDto {
    let named = named_snapshot();
    let anon = anon_snapshot();
    let named_bytes: u64 = named.iter().map(|(_, e)| e.text().len() as u64).sum();
    let anon_bytes: u64 = anon.iter().map(|(_, e)| e.text().len() as u64).sum();

    InternerDto {
        ts: now_unix_ms(),
        named: InternerKindDto {
            entries: named.len() as u64,
            bytes: named_bytes,
        },
        anonymous: InternerKindDto {
            entries: anon.len() as u64,
            bytes: anon_bytes,
        },
    }
}

pub(crate) fn collect_interner_top(n: u64) -> InternerTopDto {
    let n = clamp_top_n(n);
    let now = now_monotonic_ms();

    enum Handle {
        Named(std::sync::Arc<crate::server::NamedEntry>),
        Anon(std::sync::Arc<crate::server::AnonEntry>),
    }

    let mut combined: Vec<(u64, &'static str, usize, i64, Handle)> = Vec::new();
    for (hash, entry) in named_snapshot() {
        let bytes = entry.text().len();
        combined.push((hash, "named", bytes, -1, Handle::Named(entry)));
    }
    for (hash, entry) in anon_snapshot() {
        let idle = entry.idle_ms(now) as i64;
        let bytes = entry.text().len();
        combined.push((hash, "anonymous", bytes, idle, Handle::Anon(entry)));
    }
    combined.sort_by_key(|r| std::cmp::Reverse(r.2));

    let entries = combined
        .into_iter()
        .take(n as usize)
        .map(|(hash, kind, bytes, idle_ms, handle)| {
            let text = match handle {
                Handle::Named(e) => e.text().clone(),
                Handle::Anon(e) => e.text().clone(),
            };
            let preview = crate::utils::strings::preview_query(&text);
            InternerTopRowDto {
                hash: format!("{:#x}", hash),
                kind: kind.to_string(),
                bytes: bytes as u64,
                idle_ms,
                preview,
            }
        })
        .collect();

    InternerTopDto {
        ts: now_unix_ms(),
        n,
        entries,
    }
}
