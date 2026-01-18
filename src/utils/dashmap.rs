use dashmap::DashMap;
use std::hash::Hash;

/// Creates a new DashMap with shard count based on worker_threads.
/// This avoids overhead from incorrect CPU detection in k8s pods.
pub fn new_dashmap<K, V>(worker_threads: usize) -> DashMap<K, V>
where
    K: Eq + Hash,
{
    DashMap::with_shard_amount(optimal_shard_count(worker_threads))
}

/// Creates a new DashMap with capacity and shard count based on worker_threads.
pub fn new_dashmap_with_capacity<K, V>(capacity: usize, worker_threads: usize) -> DashMap<K, V>
where
    K: Eq + Hash,
{
    DashMap::with_capacity_and_shard_amount(capacity, optimal_shard_count(worker_threads))
}

/// Calculates optimal shard count based on worker_threads.
/// Uses power of 2 for better hash distribution.
fn optimal_shard_count(worker_threads: usize) -> usize {
    // Minimum 4 shards, maximum based on worker_threads * 4
    // Round up to nearest power of 2 for better hash distribution
    let target = (worker_threads * 4).max(4);
    target.next_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimal_shard_count() {
        // worker_threads=1 -> target=4 -> 4 shards
        assert_eq!(optimal_shard_count(1), 4);
        // worker_threads=2 -> target=8 -> 8 shards
        assert_eq!(optimal_shard_count(2), 8);
        // worker_threads=4 -> target=16 -> 16 shards
        assert_eq!(optimal_shard_count(4), 16);
        // worker_threads=8 -> target=32 -> 32 shards
        assert_eq!(optimal_shard_count(8), 32);
        // worker_threads=3 -> target=12 -> 16 shards (next power of 2)
        assert_eq!(optimal_shard_count(3), 16);
    }

    #[test]
    fn test_new_dashmap() {
        let map: DashMap<u64, String> = new_dashmap(4);
        assert!(map.is_empty());
        map.insert(1, "test".to_string());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_new_dashmap_with_capacity() {
        let map: DashMap<u64, String> = new_dashmap_with_capacity(100, 4);
        assert!(map.is_empty());
        map.insert(1, "test".to_string());
        assert_eq!(map.len(), 1);
    }
}
