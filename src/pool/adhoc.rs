//! Bounded detection of previously unseen extended-protocol Parse hashes.
//!
//! A Bloom false positive suppresses a candidate. Each shard forgets its
//! history when a quarter of its bits are set, so a previously observed hash
//! can become a candidate again after that shard resets. This measures SQL
//! novelty rather than proving that a query is only executed once.

use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;
use std::sync::{Arc, Weak};

use arc_swap::{ArcSwapOption, Guard};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use super::PoolIdentifier;
use crate::web::metrics::{adhoc_query_metrics, AdhocQueryMetrics};

const TOTAL_BITMAP_BYTES: usize = 2 * 1024 * 1024;
const SHARD_COUNT: usize = 64;
const WORDS_PER_SHARD: usize = TOTAL_BITMAP_BYTES / SHARD_COUNT / size_of::<u64>();
const BITS_PER_SHARD: usize = WORDS_PER_SHARD * u64::BITS as usize;
const INDEX_MASK: usize = BITS_PER_SHARD - 1;
const RESET_SET_BITS: usize = BITS_PER_SHARD / 4;
const PROBE_COUNT: usize = 4;

struct Shard {
    words: Box<[u64]>,
    set_bits: usize,
}

impl Shard {
    fn new() -> Self {
        Self {
            words: vec![0; WORDS_PER_SHARD].into_boxed_slice(),
            set_bits: 0,
        }
    }

    fn observe(&mut self, positions: &[usize; PROBE_COUNT]) -> (bool, bool) {
        let reset = self.set_bits >= RESET_SET_BITS;
        if reset {
            self.words.fill(0);
            self.set_bits = 0;
        }

        let candidate = !self.contains(positions);
        if candidate {
            for &position in positions {
                let word = &mut self.words[position / 64];
                let bit = 1 << (position % 64);
                if *word & bit == 0 {
                    *word |= bit;
                    self.set_bits += 1;
                }
            }
        }
        (candidate, reset)
    }

    fn contains(&self, positions: &[usize; PROBE_COUNT]) -> bool {
        positions
            .iter()
            .all(|&position| self.words[position / 64] & (1 << (position % 64)) != 0)
    }
}

/// A fixed-size Bloom bitmap with independent fullness resets per shard.
pub(crate) struct QueryNoveltyTracker {
    shards: [Mutex<Shard>; SHARD_COUNT],
    metrics: AdhocQueryMetrics,
}

impl QueryNoveltyTracker {
    fn new(user: &str, database: &str) -> Self {
        Self {
            shards: std::array::from_fn(|_| Mutex::new(Shard::new())),
            metrics: adhoc_query_metrics(user, database),
        }
    }

    /// Observe an existing Parse hash without allocating or hashing SQL again.
    /// Returns true when the Bloom bitmap considers this hash unseen.
    #[inline]
    pub(crate) fn observe(&self, hash: u64) -> bool {
        let (shard, positions) = probe_positions(hash);
        let (candidate, reset) = self.shards[shard].lock().observe(&positions);
        if reset {
            self.metrics.observe_filter_reset();
        }
        self.metrics.observe_parse(candidate);
        candidate
    }
}

impl fmt::Debug for QueryNoveltyTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryNoveltyTracker")
            .field("bitmap_bytes", &TOTAL_BITMAP_BYTES)
            .field("shards", &SHARD_COUNT)
            .finish_non_exhaustive()
    }
}

/// Shared by pool clones and retained clients; reload swaps its optional tracker.
#[derive(Debug)]
pub struct AdhocQueryControl {
    identifier: PoolIdentifier,
    tracker: ArcSwapOption<QueryNoveltyTracker>,
}

impl AdhocQueryControl {
    /// Construct a standalone control, also used by the production registry.
    pub fn new(identifier: PoolIdentifier, enabled: bool) -> Self {
        let tracker =
            enabled.then(|| Arc::new(QueryNoveltyTracker::new(&identifier.user, &identifier.db)));
        Self {
            identifier,
            tracker: ArcSwapOption::from(tracker),
        }
    }

    /// False also means that tracking is disabled; disabled calls change no metrics.
    #[inline]
    pub fn observe(&self, hash: u64) -> bool {
        match self.load().as_ref() {
            Some(tracker) => tracker.observe(hash),
            None => false,
        }
    }

    pub(crate) fn load(&self) -> Guard<Option<Arc<QueryNoveltyTracker>>> {
        self.tracker.load()
    }

    fn is_enabled(&self) -> bool {
        self.tracker.load().is_some()
    }

    /// Production calls are serialized by the registry lock. An observation
    /// already holding a tracker may finish while disabling releases it.
    fn set_enabled(&self, enabled: bool) {
        if self.is_enabled() == enabled {
            return;
        }
        let tracker = enabled.then(|| {
            Arc::new(QueryNoveltyTracker::new(
                &self.identifier.user,
                &self.identifier.db,
            ))
        });
        self.tracker.store(tracker);
    }
}

/// Registration and state changes share one lock, including newly constructed
/// dynamic pools whose configuration snapshot predates a concurrent reload.
#[derive(Default)]
struct TrackingRegistry {
    enabled: bool,
    controls: HashMap<PoolIdentifier, Weak<AdhocQueryControl>>,
}

impl TrackingRegistry {
    fn register(&mut self, identifier: PoolIdentifier) -> Arc<AdhocQueryControl> {
        if let Some(control) = self.controls.get(&identifier).and_then(Weak::upgrade) {
            return control;
        }
        self.controls
            .retain(|_, control| control.strong_count() > 0);
        let control = Arc::new(AdhocQueryControl::new(identifier.clone(), self.enabled));
        self.controls.insert(identifier, Arc::downgrade(&control));
        control
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.controls.retain(|_, control| match control.upgrade() {
            Some(control) => {
                control.set_enabled(enabled);
                true
            }
            None => false,
        });
    }
}

static TRACKING_REGISTRY: Lazy<Mutex<TrackingRegistry>> =
    Lazy::new(|| Mutex::new(TrackingRegistry::default()));

/// Reuse one control for the same logical pool while any old pool clone lives.
pub(crate) fn register_pool_tracker(identifier: PoolIdentifier) -> Arc<AdhocQueryControl> {
    TRACKING_REGISTRY.lock().register(identifier)
}

/// Commit startup/reload state after successfully constructing the pool config.
pub(crate) fn set_tracking_enabled(enabled: bool) {
    TRACKING_REGISTRY.lock().set_enabled(enabled);
}

/// The setting committed by the last successful pool configuration build.
pub(crate) fn tracking_enabled() -> bool {
    TRACKING_REGISTRY.lock().enabled
}

/// SplitMix64's avalanche finalizer, applied to the existing 64-bit SQL hash.
#[inline]
fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[inline]
fn probe_positions(hash: u64) -> (usize, [usize; PROBE_COUNT]) {
    let mixed = mix64(hash);
    // Shard bits do not constrain the lower bits used for Bloom positions.
    let shard = (mixed >> (u64::BITS - SHARD_COUNT.ilog2())) as usize;
    let first = mixed as usize & INDEX_MASK;
    // An odd stride has full period in the power-of-two bitmap, so all four
    // probes are distinct. Domain separation avoids deriving it from first.
    let step = (mix64(hash ^ 0xd1b5_4a32_d192_ed03) as usize | 1) & INDEX_MASK;
    let positions = std::array::from_fn(|i| first.wrapping_add(i * step) & INDEX_MASK);
    (shard, positions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;
    use std::thread;

    fn identifier(test: &str) -> PoolIdentifier {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        PoolIdentifier::new(
            &format!("adhoc_{test}_{}", NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            "adhoc_test_user",
        )
    }

    fn tracker(test: &str) -> QueryNoveltyTracker {
        let id = identifier(test);
        QueryNoveltyTracker::new(&id.user, &id.db)
    }

    #[test]
    fn fixed_bitmap_size_and_distinct_probes() {
        let tracker = tracker("size");
        let bytes: usize = tracker
            .shards
            .iter()
            .map(|shard| shard.lock().words.len() * size_of::<u64>())
            .sum();
        assert_eq!(bytes, 2 * 1024 * 1024);
        for hash in 0..10_000 {
            let (shard, positions) = probe_positions(hash);
            assert!(shard < SHARD_COUNT);
            for (index, &position) in positions.iter().enumerate() {
                assert!(position < BITS_PER_SHARD);
                assert!(!positions[..index].contains(&position));
            }
        }
    }

    #[test]
    fn counts_first_parse_only_and_preserves_inserted_hashes() {
        let tracker = tracker("repeat");
        assert!(tracker.observe(42));
        assert!(!tracker.observe(42));
        assert!(!tracker.observe(42));
        assert_eq!(tracker.metrics.counts(), (1, 3, 0));
        for hash in 100..1_100 {
            tracker.observe(hash);
        }
        for hash in 100..1_100 {
            assert!(!tracker.observe(hash));
        }
    }

    #[test]
    fn seeded_false_positive_rate_stays_below_one_percent_near_reset_threshold() {
        let tracker = tracker("empirical_fpr");
        const SEED: u64 = 0x4d59_5df4_d0f3_3173;
        const TARGET_SHARD: usize = 0;
        const SAMPLE_COUNT: usize = 50_000;
        // mix64 is bijective. Disjoint sequence ranges therefore guarantee
        // that every sampled hash is distinct from all inserted hashes.
        const SAMPLE_SEQUENCE_START: u64 = 1 << 32;
        let mut sequence = 0;
        let mut inserted = Vec::new();
        loop {
            let hash = mix64(SEED.wrapping_add(sequence));
            sequence += 1;
            if probe_positions(hash).0 != TARGET_SHARD {
                continue;
            }
            tracker.observe(hash);
            inserted.push(hash);
            if tracker.shards[TARGET_SHARD].lock().set_bits >= RESET_SET_BITS - PROBE_COUNT {
                break;
            }
        }
        assert!(sequence < SAMPLE_SEQUENCE_START);
        {
            let shard = tracker.shards[TARGET_SHARD].lock();
            assert!(shard.set_bits >= RESET_SET_BITS - PROBE_COUNT);
            assert!(shard.set_bits < RESET_SET_BITS);

            // Freeze the bitmap while sampling: inserting sampled misses would
            // increase occupancy and reset it, invalidating the FPR measurement.
            let mut sampled = 0;
            let mut false_positives = 0;
            let mut sequence = SAMPLE_SEQUENCE_START;
            while sampled < SAMPLE_COUNT {
                let hash = mix64(SEED.wrapping_add(sequence));
                sequence += 1;
                let (index, positions) = probe_positions(hash);
                if index != TARGET_SHARD {
                    continue;
                }
                sampled += 1;
                false_positives += usize::from(shard.contains(&positions));
            }
            assert!(false_positives > 0);
            assert!(
                false_positives < SAMPLE_COUNT / 100,
                "FPR exceeded 1%: {false_positives}/{SAMPLE_COUNT}"
            );
        }
        // All truly observed hashes remain present until the shard resets.
        for hash in inserted {
            assert!(!tracker.observe(hash));
        }
        assert_eq!(tracker.metrics.counts().2, 0);
    }

    #[test]
    fn false_positive_suppresses_candidate_without_adding_bits() {
        let tracker = tracker("collision");
        let (index, positions) = probe_positions(99);
        {
            let mut shard = tracker.shards[index].lock();
            for position in positions {
                shard.words[position / 64] |= 1 << (position % 64);
            }
            shard.set_bits = PROBE_COUNT;
        }
        assert!(!tracker.observe(99));
        assert_eq!(tracker.shards[index].lock().set_bits, PROBE_COUNT);
        assert_eq!(tracker.metrics.counts(), (0, 1, 0));
    }

    #[test]
    fn concurrent_first_observations_count_exactly_once() {
        let tracker = Arc::new(tracker("concurrent"));
        let barrier = Arc::new(Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let tracker = tracker.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    tracker.observe(0x0123_4567_89ab_cdef)
                })
            })
            .collect();
        let candidates = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|candidate| *candidate)
            .count();
        assert_eq!(candidates, 1);
        assert_eq!(tracker.metrics.counts(), (1, 16, 0));
    }

    #[test]
    fn fullness_resets_only_the_selected_shard_before_next_observation() {
        let tracker = tracker("reset");
        let full_hash = 123;
        let (full_index, _) = probe_positions(full_hash);
        let other_hash = (124..)
            .find(|hash| probe_positions(*hash).0 != full_index)
            .unwrap();
        assert!(tracker.observe(full_hash));
        assert!(tracker.observe(other_hash));
        {
            let mut shard = tracker.shards[full_index].lock();
            shard.words.fill(0);
            shard.words[..WORDS_PER_SHARD / 4].fill(u64::MAX);
            shard.set_bits = RESET_SET_BITS;
        }
        assert!(tracker.observe(full_hash));
        assert_eq!(tracker.shards[full_index].lock().set_bits, PROBE_COUNT);
        assert!(!tracker.observe(full_hash));
        assert!(!tracker.observe(other_hash));
        assert_eq!(tracker.metrics.counts(), (3, 5, 1));
    }

    #[test]
    fn controls_are_disabled_without_bitmap_or_metric_updates() {
        let id = identifier("disabled");
        let control = AdhocQueryControl::new(id.clone(), false);
        assert!(!control.is_enabled());
        assert!(control.tracker.load().is_none());
        assert!(!control.observe(42));
        assert_eq!(adhoc_query_metrics(&id.user, &id.db).counts(), (0, 0, 0));
    }

    #[test]
    fn unchanged_enable_preserves_history_and_totals_survive_reenable() {
        let id = identifier("toggle");
        let control = AdhocQueryControl::new(id.clone(), true);
        assert!(control.observe(42));
        let original = control.tracker.load_full().unwrap();
        control.set_enabled(true);
        assert!(Arc::ptr_eq(
            &original,
            &control.tracker.load_full().unwrap()
        ));
        assert!(!control.observe(42));
        control.set_enabled(false);
        assert!(!control.observe(43));
        control.set_enabled(false);
        control.set_enabled(true);
        assert!(control.observe(42));
        assert!(!Arc::ptr_eq(
            &original,
            &control.tracker.load_full().unwrap()
        ));
        assert_eq!(adhoc_query_metrics(&id.user, &id.db).counts(), (2, 3, 0));
    }

    #[test]
    fn registry_reuses_live_pool_identity_and_isolates_distinct_pools() {
        let mut registry = TrackingRegistry::default();
        registry.set_enabled(true);
        let first_id = identifier("identity");
        let first = registry.register(first_id.clone());
        let retained_pool_clone = first.clone();
        let recreated_pool = registry.register(first_id);
        assert!(Arc::ptr_eq(&retained_pool_clone, &recreated_pool));
        let different_pool = registry.register(identifier("identity_other"));
        assert!(!Arc::ptr_eq(&first, &different_pool));
        assert!(first.observe(42));
        assert!(!recreated_pool.observe(42));
        assert!(different_pool.observe(42));
        registry.set_enabled(false);
        assert!(!retained_pool_clone.is_enabled());
        assert!(!different_pool.is_enabled());
        registry.set_enabled(true);
        assert!(retained_pool_clone.is_enabled());
        assert!(different_pool.is_enabled());
    }

    #[test]
    fn registry_prunes_weak_entries_during_register_and_toggle() {
        let mut registry = TrackingRegistry::default();
        let first_id = identifier("weak_first");
        let first = registry.register(first_id.clone());
        let weak = Arc::downgrade(&first);
        drop(first);
        let second = registry.register(identifier("weak_second"));
        assert_eq!(registry.controls.len(), 1);
        assert!(!registry.controls.contains_key(&first_id));
        assert!(weak.upgrade().is_none());
        drop(second);
        registry.set_enabled(true);
        assert!(registry.controls.is_empty());
    }

    #[test]
    fn registration_racing_toggle_inherits_committed_state() {
        let registry = Arc::new(Mutex::new(TrackingRegistry::default()));
        let barrier = Arc::new(Barrier::new(17));
        let id = identifier("register_race");
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let registry = registry.clone();
                let barrier = barrier.clone();
                let id = id.clone();
                thread::spawn(move || {
                    barrier.wait();
                    registry.lock().register(id)
                })
            })
            .collect();
        barrier.wait();
        registry.lock().set_enabled(true);
        let controls: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(controls.iter().all(|control| control.is_enabled()));
        assert!(controls
            .iter()
            .all(|control| Arc::ptr_eq(control, &controls[0])));
        let after = registry.lock().register(id);
        assert!(after.is_enabled());
        assert!(Arc::ptr_eq(&after, &controls[0]));
    }
}
