use crate::admin::events::get_events_since;
use crate::web::routes::dto::{EventEntryDto, EventsDto};

use super::now_unix_ms;

pub(crate) fn collect_events(since: u64, max: u64) -> EventsDto {
    // Cap max at 1000 — protects against accidental ?max=10000 over the wire.
    const HARD_CAP: usize = 1000;
    let max_n = (max.min(HARD_CAP as u64) as usize).max(1);
    let (entries, next_seq) = get_events_since(since, max_n);

    let events: Vec<EventEntryDto> = entries
        .into_iter()
        .map(|e| EventEntryDto {
            seq: e.seq,
            ts_ms: e.ts_ms,
            target: e.target.to_string(),
            message: e.message,
        })
        .collect();

    EventsDto {
        ts: now_unix_ms(),
        next_seq,
        events,
    }
}
