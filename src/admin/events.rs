//! Bounded ring buffer of lifecycle events. Frontend reads via
//! `/api/events?since=<seq>&max=` to render vertical-line annotations
//! on Overview/Wall graphs and to drive the restart/reload notifications
//! in the sidebar. Targets currently emitted:
//!
//! * `PROCESS_START` — pushed once when the binary finishes setup.
//!   Carries pid and version so a fresh tab opening the UI can
//!   reconcile its cached identity without waiting for the first
//!   `/api/overview` poll.
//! * `RELOAD` / `PAUSE` / `RESUME` / `RECONNECT` — admin commands
//!   (`src/admin/commands.rs`, `src/admin/operations.rs`).
//! * `CONFIG_VALIDATION_ERROR` — pushed when a config reload (admin
//!   RELOAD or `SIGHUP`) is rejected by `Config::validate`. Carries
//!   the validator's message so the operator sees *why* the new
//!   config did not take effect.
//!
//! This module is intentionally simple: a single Mutex<VecDeque>, no
//! lock-free fancy. Admin commands fire on the order of one per few
//! minutes per cluster; contention is negligible. Reads come from a
//! handful of Web UI operators per hour.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of events retained in the ring. At one admin command
/// every few minutes this is well over a day of history; oldest events
/// are dropped silently when the buffer fills.
const BUFFER_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub struct EventEntry {
    /// Monotonically increasing sequence number assigned at push time.
    pub seq: u64,
    /// Wall-clock timestamp in milliseconds since unix epoch.
    pub ts_ms: u64,
    /// One of `"PROCESS_START"`, `"RELOAD"`, `"PAUSE"`, `"RESUME"`,
    /// `"RECONNECT"`, `"CONFIG_VALIDATION_ERROR"`. Treat as an open enum:
    /// the frontend maps unknown targets to a neutral chip rather than
    /// failing, so adding a new target on the backend never breaks an
    /// older UI build.
    pub target: &'static str,
    /// Human-readable description of the event (e.g. `"pool main@db1 paused"`).
    pub message: String,
}

static SEQ_COUNTER: AtomicU64 = AtomicU64::new(1);

fn buffer() -> &'static Mutex<VecDeque<EventEntry>> {
    static BUFFER: OnceLock<Mutex<VecDeque<EventEntry>>> = OnceLock::new();
    BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(BUFFER_CAPACITY)))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Records an admin event in the ring. Called from `src/admin/commands.rs`
/// after a successful command.
pub fn push_event(target: &'static str, message: String) {
    let entry = EventEntry {
        seq: SEQ_COUNTER.fetch_add(1, Ordering::Relaxed),
        ts_ms: now_ms(),
        target,
        message,
    };
    if let Ok(mut buf) = buffer().lock() {
        if buf.len() >= BUFFER_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

/// Returns all events with `seq > since`, capped at `max` entries, plus the
/// next sequence number an operator should poll with. Empty when no events
/// have been pushed.
pub fn get_events_since(since: u64, max: usize) -> (Vec<EventEntry>, u64) {
    let buf = match buffer().lock() {
        Ok(b) => b,
        Err(_) => return (Vec::new(), since),
    };
    let mut next_seq = since;
    let entries: Vec<EventEntry> = buf
        .iter()
        .filter(|e| e.seq > since)
        .take(max)
        .map(|e| {
            next_seq = next_seq.max(e.seq);
            e.clone()
        })
        .collect();
    (entries, next_seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Tests touch the global SEQ_COUNTER and BUFFER, so serialise them.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: OnceLock<StdMutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| StdMutex::new(())).lock().unwrap()
    }

    fn drain() {
        let _ = buffer().lock().map(|mut b| b.clear());
    }

    #[test]
    fn push_event_appends_with_increasing_seq() {
        let _g = lock();
        drain();
        push_event("RELOAD", "config reloaded".into());
        push_event("PAUSE", "pool main@db1 paused".into());
        let (events, next) = get_events_since(0, 100);
        assert_eq!(events.len(), 2);
        assert!(events[0].seq < events[1].seq);
        assert_eq!(next, events[1].seq);
        assert_eq!(events[0].target, "RELOAD");
        assert_eq!(events[1].target, "PAUSE");
    }

    #[test]
    fn get_events_since_filters_by_seq() {
        let _g = lock();
        drain();
        push_event("RELOAD", "first".into());
        push_event("PAUSE", "second".into());
        let (first_batch, next1) = get_events_since(0, 100);
        let (after_first, next2) = get_events_since(first_batch[0].seq, 100);
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].message, "second");
        assert_eq!(next2, next1);
    }

    #[test]
    fn get_events_since_respects_max() {
        let _g = lock();
        drain();
        for i in 0..10 {
            push_event("RELOAD", format!("event {i}"));
        }
        let (events, _) = get_events_since(0, 3);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn buffer_drops_oldest_when_full() {
        let _g = lock();
        drain();
        for i in 0..(BUFFER_CAPACITY + 50) {
            push_event("RELOAD", format!("event {i}"));
        }
        let (events, _) = get_events_since(0, BUFFER_CAPACITY * 2);
        assert_eq!(events.len(), BUFFER_CAPACITY);
        // Earliest 50 dropped — first surviving event is index 50 (message "event 50").
        assert_eq!(events[0].message, "event 50");
    }
}
