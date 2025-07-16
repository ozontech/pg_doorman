/// Calculates a percentile value from a pre-sorted array of samples.
///
/// This function computes the specified percentile from a sorted array of values
/// using the nearest-rank method. The percentile represents the value below which
/// a given percentage of observations fall.
///
/// # Arguments
///
/// * `sorted_samples` - A slice of u64 values that must already be sorted in ascending order
/// * `pct` - The percentile to calculate (0.0 to 100.0)
///
/// # Returns
///
/// * The value at the specified percentile, or 0 if the input array is empty
pub fn percentile_of_sorted(sorted_samples: &[u64], pct: f64) -> u64 {
    // Handle empty array case
    if sorted_samples.is_empty() {
        return 0;
    }

    // Handle single-element array case
    if sorted_samples.len() == 1 {
        return sorted_samples[0];
    }

    // Handle edge cases for percentiles
    if pct <= 0.0 {
        return sorted_samples[0]; // Return minimum value for 0th percentile
    }
    if pct >= 100.0 {
        return sorted_samples[sorted_samples.len() - 1]; // Return maximum value for 100th percentile
    }

    // Calculate the rank (position) for the requested percentile
    // The formula converts the percentile (0-100) to a position in the array
    let rank = (pct * sorted_samples.len() as f64 / 100.0).ceil() as usize;

    // Ensure rank is at least 1 and at most the length of the array
    let rank = rank.max(1).min(sorted_samples.len());

    // Return the value at the calculated rank (adjusting for 0-based indexing)
    sorted_samples[rank - 1]
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_basic_percentile() {
        let mut val = vec![0, 1, 2, 8, 9, 3, 4, 5, 6, 7];
        let times = val.as_mut_slice();
        times.sort();
        assert_eq!(percentile_of_sorted(times, 10.0), 0);
        assert_eq!(percentile_of_sorted(times, 50.0), 4);
        assert_eq!(percentile_of_sorted(times, 99.0), 9);
    }

    #[test]
    fn test_edge_cases() {
        // Empty array
        let empty: Vec<u64> = vec![];
        assert_eq!(percentile_of_sorted(&empty, 50.0), 0);

        // Single element array
        let single = vec![42];
        assert_eq!(percentile_of_sorted(&single, 0.0), 42);
        assert_eq!(percentile_of_sorted(&single, 50.0), 42);
        assert_eq!(percentile_of_sorted(&single, 100.0), 42);
    }

    #[test]
    fn test_various_percentiles() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        // Test boundary percentiles
        assert_eq!(percentile_of_sorted(&data, 0.0), 1); // 0th percentile (minimum)
        assert_eq!(percentile_of_sorted(&data, 100.0), 10); // 100th percentile (maximum)

        // Test quartiles
        assert_eq!(percentile_of_sorted(&data, 25.0), 3); // 1st quartile
        assert_eq!(percentile_of_sorted(&data, 50.0), 5); // 2nd quartile (median)
        assert_eq!(percentile_of_sorted(&data, 75.0), 8); // 3rd quartile

        // Test other common percentiles
        assert_eq!(percentile_of_sorted(&data, 90.0), 9); // 90th percentile
        assert_eq!(percentile_of_sorted(&data, 95.0), 10); // 95th percentile
    }

    #[test]
    fn test_large_array() {
        // Create a large array with 1000 elements
        let large_array: Vec<u64> = (1..=1000).collect();

        assert_eq!(percentile_of_sorted(&large_array, 0.0), 1); // Minimum
        assert_eq!(percentile_of_sorted(&large_array, 50.0), 500); // Median
        assert_eq!(percentile_of_sorted(&large_array, 90.0), 900); // 90th percentile
        assert_eq!(percentile_of_sorted(&large_array, 99.0), 990); // 99th percentile
        assert_eq!(percentile_of_sorted(&large_array, 100.0), 1000); // Maximum
    }

    #[test]
    fn test_unusual_percentiles() {
        let data = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100];

        // Test non-integer percentiles
        assert_eq!(percentile_of_sorted(&data, 12.5), 20); // Between 10% and 20%
        assert_eq!(percentile_of_sorted(&data, 33.3), 40); // About 1/3
        assert_eq!(percentile_of_sorted(&data, 66.7), 70); // About 2/3

        // Test very small and very large percentiles
        assert_eq!(percentile_of_sorted(&data, 0.1), 10); // Very small percentile
        assert_eq!(percentile_of_sorted(&data, 99.9), 100); // Very large percentile
    }
}
