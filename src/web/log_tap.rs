//! Lock-free LogTap: in-memory subset of log records served via /api/logs.
//!
//! Design (spec section 7.3, 9):
//! - Producer (`log::Log::log` hook) reads `tap_active` AtomicBool gate.
//!   When off, exits in ~1 ns. When on, formats record into a bounded
//!   buffer (4 KB cap, UTF-8 safe truncation) and `try_send` into a
//!   bounded MPSC. Drop-new on channel-full keeps the SQL hot path
//!   free of allocation-spikes from megabyte debug deparse output.
//! - Consumer is a single tokio task, the sole owner of the VecDeque.
//!   Assigns monotonic `seq`, processes `Drain` commands by cloning a
//!   filtered subset; never blocks producers.
//! - Reaper task disables the tap after 2 min without GETs.

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use log::{Level, Record};
use once_cell::sync::Lazy;
use tokio::sync::{mpsc, oneshot};

// 4 KB keeps a megabyte-sized debug deparse line from filling the ring with
// one entry; longer messages are truncated at a UTF-8 boundary with a marker.
const PER_ENTRY_BYTE_CAP: usize = 4 * 1024;

// Disable the tap after this much time without GETs — long enough that a
// stepped-away operator coming back from coffee still has the buffer alive,
// short enough to release memory once nobody is reading. Two minutes is the
// operator-feedback default; the tap re-arms instantly on the next request.
const IDLE_DISABLE_MS: u64 = 120_000;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub seq: u64,
    pub ts_ms: u64,
    pub level: Level,
    pub target: String,
    pub message: String,
}

/// What the producer hands to the consumer through the MPSC. seq is set
/// by the consumer (sole writer); ts_ms/level/target/message by the producer.
struct RawEntry {
    ts_ms: u64,
    level: Level,
    target: String,
    message: String,
}

pub enum TapCommand {
    Drain {
        since: u64,
        max: usize,
        level: Option<Level>,
        target: Option<String>,
        reply: oneshot::Sender<DrainResult>,
    },
    Shutdown,
}

pub struct DrainResult {
    pub entries: Vec<LogEntry>,
    pub next_seq: u64,
    pub dropped_before: u64,
    pub used_entries: usize,
}

pub struct LogTap {
    tx: mpsc::Sender<RawEntry>,
    /// Combined drop counter — bumped on producer-side `try_send` failures
    /// when the channel is full *and* on consumer-side ring-buffer
    /// evictions when a viewer is too slow. Operators read this as a
    /// single "messages I never saw" number.
    pub(crate) dropped_total: Arc<AtomicU64>,
    cmd_tx: mpsc::Sender<TapCommand>,
    pub(crate) last_request_at: Arc<AtomicU64>,
}

impl LogTap {
    /// Sends a Drain command to the consumer task and waits for the reply.
    /// Returns `Err` only when the consumer is gone — handlers turn that
    /// into a 200-with-empty-envelope so the operator's poll loop survives
    /// a momentary lapse.
    pub(crate) async fn drain(
        &self,
        since: u64,
        max: usize,
        level: Option<Level>,
        target: Option<String>,
    ) -> Result<DrainResult, ()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(TapCommand::Drain {
                since,
                max,
                level,
                target,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ())?;
        reply_rx.await.map_err(|_| ())
    }
}

/// Module-level statics — single global LogTap. The gate is private to
/// this module so callers cannot toggle it without going through
/// `enable_log_tap` / `disable_log_tap` and desyncing it from `LOG_TAP`.
static TAP_ACTIVE: AtomicBool = AtomicBool::new(false);
static LOG_TAP: Lazy<ArcSwap<Option<Arc<LogTap>>>> = Lazy::new(|| ArcSwap::from_pointee(None));
static LIFECYCLE: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Monotonic millisecond clock anchored at the first call.
pub(crate) fn now_monotonic_ms() -> u64 {
    static START: Lazy<Instant> = Lazy::new(Instant::now);
    START.elapsed().as_millis() as u64
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Returns the current LogTap if active, otherwise None. Uses
/// `ArcSwap::load` rather than `load_full`: the outer `Arc<Option<…>>`
/// stays inside the hazard-pointer guard for the lifetime of the call,
/// so we pay one inner-Arc clone per producer push instead of two
/// when the tap is active.
pub fn log_tap() -> Option<Arc<LogTap>> {
    LOG_TAP.load().as_ref().clone()
}

struct BoundedWriter {
    buf: String,
    cap: usize,
    overflow: bool,
}

impl BoundedWriter {
    fn new(cap: usize) -> Self {
        // Pre-allocate up to 512 bytes — covers the median pg_doorman log
        // line without a regrow, and stays well under PER_ENTRY_BYTE_CAP
        // even if cap is large. Without this hint each push() reallocs
        // a couple of times as the format! writer grows the String.
        Self {
            buf: String::with_capacity(cap.min(512)),
            cap,
            overflow: false,
        }
    }
    fn finish(mut self) -> String {
        if self.overflow {
            self.buf.push_str("…<truncated>");
        }
        self.buf
    }
}

impl fmt::Write for BoundedWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let remaining = self.cap.saturating_sub(self.buf.len());
        if remaining == 0 {
            self.overflow = true;
            return Ok(());
        }
        if s.len() <= remaining {
            self.buf.push_str(s);
        } else {
            let mut end = remaining;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            self.buf.push_str(&s[..end]);
            self.overflow = true;
        }
        Ok(())
    }
}

/// Producer-side hook called from `LogLevelController::log`. Returns
/// immediately when `TAP_ACTIVE` is false (one Acquire load, ~1 ns on x86).
/// Otherwise formats into a bounded buffer and try_sends through the
/// MPSC. On send failure (channel full or closed) the drop is counted
/// in `dropped_total`.
pub fn push(record: &Record) {
    if !TAP_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let Some(tap) = log_tap() else {
        return;
    };

    let mut bw = BoundedWriter::new(PER_ENTRY_BYTE_CAP);
    let _ = write!(&mut bw, "{}", record.args());
    let message = bw.finish();

    let raw = RawEntry {
        ts_ms: now_unix_ms(),
        level: record.level(),
        target: record.target().to_string(),
        message,
    };

    if tap.tx.try_send(raw).is_err() {
        tap.dropped_total.fetch_add(1, Ordering::Relaxed);
    }
}

struct ConsumerState {
    entries: VecDeque<LogEntry>,
    next_seq: u64,
    max_entries: usize,
}

async fn run_consumer(
    mut rx: mpsc::Receiver<RawEntry>,
    mut cmd_rx: mpsc::Receiver<TapCommand>,
    dropped: Arc<AtomicU64>,
    max_entries: usize,
) {
    let mut s = ConsumerState {
        entries: VecDeque::with_capacity(max_entries),
        next_seq: 0,
        max_entries,
    };

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(TapCommand::Drain { since, max, level, target, reply }) => {
                        let mut entries: Vec<LogEntry> = Vec::new();
                        for e in s.entries.iter() {
                            if e.seq < since {
                                continue;
                            }
                            if let Some(min_lvl) = level {
                                if e.level > min_lvl {
                                    continue;
                                }
                            }
                            if let Some(t) = &target {
                                if !e.target.contains(t.as_str()) {
                                    continue;
                                }
                            }
                            entries.push(e.clone());
                            if entries.len() >= max {
                                break;
                            }
                        }
                        let front_seq = s.entries.front().map(|e| e.seq).unwrap_or(s.next_seq);
                        let dropped_before = front_seq.saturating_sub(since);
                        let _ = reply.send(DrainResult {
                            entries,
                            next_seq: s.next_seq,
                            dropped_before,
                            used_entries: s.entries.len(),
                        });
                    }
                    Some(TapCommand::Shutdown) | None => break,
                }
            }
            raw = rx.recv() => {
                let Some(raw) = raw else { break; };
                let entry = LogEntry {
                    seq: s.next_seq,
                    ts_ms: raw.ts_ms,
                    level: raw.level,
                    target: raw.target,
                    message: raw.message,
                };
                s.next_seq += 1;
                s.entries.push_back(entry);
                while s.entries.len() > s.max_entries {
                    s.entries.pop_front();
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// Activates the tap. Idempotent — if already active, returns the existing Arc.
/// Spawns the consumer task on first activation. Lifecycle mutex serialises
/// activation/reaping; this path is admin-triggered and never on the hot path.
pub fn enable_log_tap(max_entries: usize) -> Arc<LogTap> {
    let _g = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());

    if TAP_ACTIVE.load(Ordering::Relaxed) {
        if let Some(arc) = log_tap() {
            return arc;
        }
    }

    let dropped_total = Arc::new(AtomicU64::new(0));
    let last_request_at = Arc::new(AtomicU64::new(now_monotonic_ms()));
    // Floor of 64 keeps the channel from collapsing if config sets
    // log_tap_max_entries near zero.
    let max = max_entries.max(64);
    let (tx, rx) = mpsc::channel(max);
    // 8 cmd slots: requests are serialised by LIFECYCLE; slack for rapid
    // enable/disable cycles.
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    let dropped_for_consumer = dropped_total.clone();
    tokio::spawn(async move {
        run_consumer(rx, cmd_rx, dropped_for_consumer, max).await;
    });

    let arc = Arc::new(LogTap {
        tx,
        dropped_total,
        cmd_tx,
        last_request_at,
    });

    LOG_TAP.store(Arc::new(Some(arc.clone())));
    TAP_ACTIVE.store(true, Ordering::Release);

    arc
}

/// Deactivates the tap. Drops the producer-side senders so the consumer
/// task exits cleanly. New events while disabled are dropped at the gate.
pub fn disable_log_tap() {
    let _g = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    TAP_ACTIVE.store(false, Ordering::Release);
    LOG_TAP.store(Arc::new(None));
}

/// Reaper task: disables the tap after 2 min without /api/logs traffic.
/// Spawned once during `start_web_server` startup when ui_active is true
/// and log_tap_max_entries > 0.
pub async fn run_reaper() {
    // 5 s tick: catches the IDLE_DISABLE_MS deadline within one extra cycle.
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.tick().await; // skip the first tick which fires immediately
    loop {
        interval.tick().await;
        if let Some(tap) = log_tap() {
            let last = tap.last_request_at.load(Ordering::Relaxed);
            if now_monotonic_ms().saturating_sub(last) > IDLE_DISABLE_MS {
                disable_log_tap();
                log::debug!("LogTap disabled (no consumers for 2 minutes)");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn bounded_writer_truncates_at_cap() {
        let mut bw = BoundedWriter::new(10);
        let _ = write!(&mut bw, "0123456789ABCDEF");
        let s = bw.finish();
        assert!(s.starts_with("0123456789"));
        assert!(s.contains("…<truncated>"));
    }

    #[test]
    fn bounded_writer_short_input_no_marker() {
        let mut bw = BoundedWriter::new(100);
        let _ = write!(&mut bw, "short");
        let s = bw.finish();
        assert_eq!(s, "short");
    }

    #[test]
    fn bounded_writer_utf8_safe_truncation() {
        let mut bw = BoundedWriter::new(4);
        // emoji takes 4 bytes — first push fits exactly; second pushes overflow.
        let _ = write!(&mut bw, "🎉🎉");
        let s = bw.finish();
        // First emoji intact, second dropped, marker added.
        assert!(s.starts_with("🎉"));
        assert!(s.contains("…<truncated>"));
        assert!(!s.contains("\u{FFFD}"));
    }

    #[test]
    #[serial]
    fn push_does_not_panic_when_inactive() {
        // Without env_logger::init() the global log macros are a no-op,
        // so this test is a smoke guard against the gate panicking, not
        // an end-to-end check of the gate. The gate logic itself is
        // covered indirectly by enable_disable_roundtrip and the consumer
        // unit tests in later tasks.
        TAP_ACTIVE.store(false, Ordering::Relaxed);
        log::info!("safety check — should be ignored");
    }

    #[tokio::test]
    #[serial]
    async fn enable_disable_roundtrip() {
        // Idempotent: a second enable_log_tap while active returns the existing Arc.
        disable_log_tap();
        let a = enable_log_tap(64);
        let b = enable_log_tap(64);
        assert!(Arc::ptr_eq(&a, &b));
        disable_log_tap();
        assert!(log_tap().is_none());
    }
}
