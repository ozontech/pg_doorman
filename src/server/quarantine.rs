//! Per-pool quarantine for operator-supplied startup parameters that PG keeps
//! rejecting at backend startup.
//!
//! Lifecycle:
//! 1. Backend `Server::startup` issues StartupMessage with N parameters.
//! 2. PG ErrorResponse arrives; parsed parameter name is reported via
//!    [`QuarantineState::record_rejection`].
//! 3. After N consecutive rejections of the same parameter
//!    (`startup_parameter_quarantine_threshold`), the key is quarantined for
//!    TTL ms.
//! 4. [`QuarantineState::filter_active_keys`] is called on every subsequent
//!    backend spawn and skips quarantined keys.
//! 5. TTL only releases keys (never a "success": we skipped them, so success
//!    is meaningless evidence).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct QuarantineEntry {
    pub reject_count: u32,
    pub quarantined_until: Option<Instant>,
    pub last_sqlstate: String,
}

#[derive(Debug)]
pub struct QuarantineState {
    threshold: u32,
    ttl: Duration,
    entries: Mutex<HashMap<String, QuarantineEntry>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Counter advanced but threshold not yet reached.
    Counting { reject_count: u32 },
    /// This call moved the parameter into quarantine.
    JustQuarantined,
    /// Already quarantined; this is a refresher with a fresh deadline.
    AlreadyQuarantined,
}

impl QuarantineState {
    pub fn new(threshold: u32, ttl: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Record a backend-startup rejection for `key`. Returns the resulting
    /// outcome for caller-side logging and metrics.
    pub fn record_rejection(&self, key: &str, sqlstate: &str) -> RecordOutcome {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("quarantine mutex");
        let entry = entries
            .entry(key.to_owned())
            .or_insert_with(|| QuarantineEntry {
                reject_count: 0,
                quarantined_until: None,
                last_sqlstate: sqlstate.to_owned(),
            });
        entry.last_sqlstate = sqlstate.to_owned();
        if entry.quarantined_until.map(|d| d > now).unwrap_or(false) {
            entry.quarantined_until = Some(now + self.ttl);
            return RecordOutcome::AlreadyQuarantined;
        }
        entry.reject_count = entry.reject_count.saturating_add(1);
        if entry.reject_count >= self.threshold {
            entry.quarantined_until = Some(now + self.ttl);
            return RecordOutcome::JustQuarantined;
        }
        RecordOutcome::Counting {
            reject_count: entry.reject_count,
        }
    }

    /// Strip currently-quarantined keys from the operator-supplied map and
    /// drop the bookkeeping for entries whose TTL has expired (so the next
    /// rejection starts counting afresh).
    ///
    /// Returns the list of keys whose quarantine just expired in this call;
    /// callers should use it to clear corresponding Prometheus gauges.
    pub fn filter_active_keys(
        &self,
        params: &mut std::collections::BTreeMap<String, String>,
    ) -> Vec<String> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("quarantine mutex");
        let mut released: Vec<String> = Vec::new();
        let mut to_drop: Vec<String> = Vec::new();
        for (key, entry) in entries.iter_mut() {
            match entry.quarantined_until {
                Some(deadline) if deadline > now => {
                    params.remove(key);
                }
                Some(_) => {
                    entry.quarantined_until = None;
                    entry.reject_count = 0;
                    to_drop.push(key.clone());
                    released.push(key.clone());
                }
                None => {}
            }
        }
        for k in &to_drop {
            entries.remove(k);
        }
        released
    }

    /// Return currently-quarantined parameter names (for SHOW POOLS / metrics).
    pub fn snapshot_quarantined(&self) -> Vec<String> {
        let now = Instant::now();
        let entries = self.entries.lock().expect("quarantine mutex");
        entries
            .iter()
            .filter_map(|(k, e)| match e.quarantined_until {
                Some(d) if d > now => Some(k.clone()),
                _ => None,
            })
            .collect()
    }

    /// Reset the partial-rejection counters for keys that pg_doorman just
    /// successfully sent in a backend StartupMessage (the backend reached
    /// `ReadyForQuery`). This keeps the threshold model honest: only N
    /// *consecutive* rejections of the same key arm quarantine, not N
    /// rejections spread across a sea of healthy startups.
    ///
    /// Keys currently quarantined (`quarantined_until = Some(_)`) are not
    /// touched; their TTL is the only release path. By definition such keys
    /// were not in the sent map at all, so a successful startup says nothing
    /// about whether the underlying problem with them was fixed.
    pub fn record_success(&self, sent_keys: &std::collections::BTreeMap<String, String>) {
        if sent_keys.is_empty() {
            return;
        }
        let mut entries = self.entries.lock().expect("quarantine mutex");
        for k in sent_keys.keys() {
            if let Some(entry) = entries.get_mut(k) {
                if entry.quarantined_until.is_none() {
                    entry.reject_count = 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn first_rejections_count_up() {
        let q = QuarantineState::new(3, Duration::from_secs(60));
        assert_eq!(
            q.record_rejection("foo", "42704"),
            RecordOutcome::Counting { reject_count: 1 }
        );
        assert_eq!(
            q.record_rejection("foo", "42704"),
            RecordOutcome::Counting { reject_count: 2 }
        );
        assert_eq!(
            q.record_rejection("foo", "42704"),
            RecordOutcome::JustQuarantined
        );
    }

    #[test]
    fn extra_rejection_after_quarantine_is_refresher() {
        let q = QuarantineState::new(2, Duration::from_secs(60));
        let _ = q.record_rejection("foo", "42704");
        let _ = q.record_rejection("foo", "42704");
        assert_eq!(
            q.record_rejection("foo", "42704"),
            RecordOutcome::AlreadyQuarantined
        );
    }

    #[test]
    fn filter_strips_active_quarantines() {
        let q = QuarantineState::new(1, Duration::from_secs(60));
        let _ = q.record_rejection("bad", "42704");
        let mut params: BTreeMap<String, String> = [
            ("bad".to_string(), "x".to_string()),
            ("ok".to_string(), "y".to_string()),
        ]
        .into_iter()
        .collect();
        let released = q.filter_active_keys(&mut params);
        assert!(!params.contains_key("bad"));
        assert!(params.contains_key("ok"));
        assert!(released.is_empty(), "active quarantine should not release");
    }

    #[test]
    fn expired_quarantine_is_released_and_counter_resets() {
        let q = QuarantineState::new(1, Duration::from_millis(10));
        let _ = q.record_rejection("bad", "42704");
        std::thread::sleep(Duration::from_millis(25));
        let mut params: BTreeMap<String, String> =
            [("bad".to_string(), "x".to_string())].into_iter().collect();
        let released = q.filter_active_keys(&mut params);
        assert!(
            params.contains_key("bad"),
            "expired quarantine should not strip the key"
        );
        assert_eq!(released, vec!["bad".to_string()]);
        // counter reset; next rejection re-starts counting and (with threshold=1)
        // immediately quarantines.
        assert_eq!(
            q.record_rejection("bad", "42704"),
            RecordOutcome::JustQuarantined
        );
    }

    #[test]
    fn snapshot_lists_only_active() {
        let q = QuarantineState::new(1, Duration::from_secs(60));
        let _ = q.record_rejection("a", "42704");
        let _ = q.record_rejection("b", "22023");
        let mut s = q.snapshot_quarantined();
        s.sort();
        assert_eq!(s, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn threshold_zero_is_clamped_to_one() {
        let q = QuarantineState::new(0, Duration::from_secs(60));
        assert_eq!(
            q.record_rejection("foo", "42704"),
            RecordOutcome::JustQuarantined
        );
    }

    #[test]
    fn record_success_resets_counter_for_non_quarantined_key() {
        let q = QuarantineState::new(3, Duration::from_secs(60));
        // counter = 1, not yet quarantined.
        let _ = q.record_rejection("a", "42704");
        let sent: BTreeMap<String, String> =
            [("a".to_string(), "x".to_string())].into_iter().collect();
        q.record_success(&sent);
        // Next rejection counts from 1 again, not 2 (counter was reset).
        assert_eq!(
            q.record_rejection("a", "42704"),
            RecordOutcome::Counting { reject_count: 1 }
        );
    }

    #[test]
    fn record_success_preserves_quarantine_state() {
        let q = QuarantineState::new(1, Duration::from_secs(60));
        // threshold=1 -> already quarantined.
        let _ = q.record_rejection("a", "42704");
        let sent: BTreeMap<String, String> =
            [("a".to_string(), "x".to_string())].into_iter().collect();
        // Success on the same key (e.g., somebody else removed the bad value).
        // record_success must not touch the quarantine deadline because the
        // key was never actually sent in this successful startup.
        q.record_success(&sent);
        assert_eq!(q.snapshot_quarantined(), vec!["a".to_string()]);
    }

    #[test]
    fn record_success_ignores_unknown_keys() {
        let q = QuarantineState::new(3, Duration::from_secs(60));
        let _ = q.record_rejection("known", "42704");
        let sent: BTreeMap<String, String> = [("never_seen".to_string(), "x".to_string())]
            .into_iter()
            .collect();
        q.record_success(&sent);
        // "known" counter untouched: next rejection counts to 2.
        assert_eq!(
            q.record_rejection("known", "42704"),
            RecordOutcome::Counting { reject_count: 2 }
        );
    }
}
