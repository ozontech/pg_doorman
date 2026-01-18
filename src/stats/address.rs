use hdrhistogram::Histogram;
use parking_lot::Mutex;
use std::sync::atomic::*;

/// Fields for tracking various statistics related to PostgreSQL connections by address.
///
/// Each field is an atomic counter allowing safe sharing and updating
/// across multiple threads without additional reference counting.
#[derive(Debug, Default)]
pub struct AddressStatFields {
    /// Number of transactions processed
    pub xact_count: AtomicU64,

    /// Number of queries processed
    pub query_count: AtomicU64,

    /// Total bytes received from clients
    pub bytes_received: AtomicU64,

    /// Total bytes sent to clients
    pub bytes_sent: AtomicU64,

    /// Total transaction processing time in microseconds
    pub xact_time_microseconds: AtomicU64,

    /// Total query processing time in microseconds
    pub query_time_microseconds: AtomicU64,

    /// Total time spent waiting for resources in microseconds
    pub wait_time: AtomicU64,

    /// Number of errors encountered
    pub errors: AtomicU64,
}

/// Maximum trackable time in microseconds for HDR histogram (10 minutes)
const HISTOGRAM_MAX_VALUE_US: u64 = 10 * 60 * 1_000_000;

/// Number of significant digits for HDR histogram precision (3 = 0.1% error)
const HISTOGRAM_SIGFIG: u8 = 2;

/// Creates a new HDR histogram for tracking latencies
fn new_histogram() -> Histogram<u64> {
    Histogram::<u64>::new_with_max(HISTOGRAM_MAX_VALUE_US, HISTOGRAM_SIGFIG)
        .expect("Failed to create histogram")
}

/// Statistics for PostgreSQL connections grouped by address.
///
/// This struct maintains three sets of statistics:
/// - `total`: Cumulative statistics since the start of the server
/// - `current`: Statistics for the current reporting period
/// - `averages`: Average values calculated from the current period
///
/// It uses HDR histograms for efficient percentile calculations with minimal memory.
#[derive(Debug)]
pub struct AddressStats {
    /// Cumulative statistics since the start of the server
    pub total: AddressStatFields,

    /// Statistics for the current reporting period (reset periodically)
    pub current: AddressStatFields,

    /// Average values calculated from the current period
    pub averages: AddressStatFields,

    /// Flag indicating if the averages have been updated since the last reporting
    pub averages_updated: AtomicBool,

    /// HDR histogram for transaction times in microseconds (reset each period)
    pub xact_histogram: Mutex<Histogram<u64>>,

    /// HDR histogram for query times in microseconds (reset each period)
    pub query_histogram: Mutex<Histogram<u64>>,
}

impl Default for AddressStats {
    fn default() -> Self {
        Self {
            total: AddressStatFields::default(),
            current: AddressStatFields::default(),
            averages: AddressStatFields::default(),
            averages_updated: AtomicBool::new(false),
            xact_histogram: Mutex::new(new_histogram()),
            query_histogram: Mutex::new(new_histogram()),
        }
    }
}

/// Implementation of IntoIterator for AddressStats to convert statistics into name-value pairs.
///
/// This allows the statistics to be easily formatted for reporting or display purposes.
/// The values are converted to f64 for consistent representation.
impl IntoIterator for &AddressStats {
    type Item = (String, f64);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    /// Converts the AddressStats into an iterator of (name, value) pairs.
    ///
    /// Total transaction and query times are converted from microseconds to milliseconds
    /// for better readability.
    fn into_iter(self) -> Self::IntoIter {
        vec![
            // Total statistics
            (
                "total_xact_count".to_string(),
                self.total.xact_count.load(Ordering::Relaxed) as f64,
            ),
            (
                "total_query_count".to_string(),
                self.total.query_count.load(Ordering::Relaxed) as f64,
            ),
            (
                "total_received".to_string(),
                self.total.bytes_received.load(Ordering::Relaxed) as f64,
            ),
            (
                "total_sent".to_string(),
                self.total.bytes_sent.load(Ordering::Relaxed) as f64,
            ),
            (
                "total_xact_time".to_string(),
                // Convert microseconds to milliseconds for better readability
                self.total.xact_time_microseconds.load(Ordering::Relaxed) as f64 / 1_000f64,
            ),
            (
                "total_query_time".to_string(),
                // Convert microseconds to milliseconds for better readability
                self.total.query_time_microseconds.load(Ordering::Relaxed) as f64 / 1_000f64,
            ),
            (
                "total_wait_time".to_string(),
                self.total.wait_time.load(Ordering::Relaxed) as f64,
            ),
            (
                "total_errors".to_string(),
                self.total.errors.load(Ordering::Relaxed) as f64,
            ),
            // Average statistics
            (
                "avg_xact_count".to_string(),
                self.averages.xact_count.load(Ordering::Relaxed) as f64,
            ),
            (
                "avg_query_count".to_string(),
                self.averages.query_count.load(Ordering::Relaxed) as f64,
            ),
            (
                "avg_recv".to_string(),
                self.averages.bytes_received.load(Ordering::Relaxed) as f64,
            ),
            (
                "avg_sent".to_string(),
                self.averages.bytes_sent.load(Ordering::Relaxed) as f64,
            ),
            (
                "avg_errors".to_string(),
                self.averages.errors.load(Ordering::Relaxed) as f64,
            ),
            (
                "avg_xact_time".to_string(),
                self.averages.xact_time_microseconds.load(Ordering::Relaxed) as f64,
            ),
            (
                "avg_query_time".to_string(),
                self.averages
                    .query_time_microseconds
                    .load(Ordering::Relaxed) as f64,
            ),
            (
                "avg_wait_time".to_string(),
                self.averages.wait_time.load(Ordering::Relaxed) as f64,
            ),
        ]
        .into_iter()
    }
}

impl AddressStats {
    /// Increments the transaction count in both total and current statistics.
    ///
    /// This method is called whenever a new transaction is started.
    #[inline(always)]
    pub fn xact_count_add(&self) {
        self.total.xact_count.fetch_add(1, Ordering::Relaxed);
        self.current.xact_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the query count in both total and current statistics.
    ///
    /// This method is called whenever a new query is executed.
    #[inline(always)]
    pub fn query_count_add(&self) {
        self.total.query_count.fetch_add(1, Ordering::Relaxed);
        self.current.query_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Adds the specified number of bytes to the received bytes counter.
    ///
    /// This method is called whenever data is received from a client.
    ///
    /// # Arguments
    ///
    /// * `bytes` - The number of bytes received
    #[inline(always)]
    pub fn bytes_received_add(&self, bytes: u64) {
        self.total
            .bytes_received
            .fetch_add(bytes, Ordering::Relaxed);
        self.current
            .bytes_received
            .fetch_add(bytes, Ordering::Relaxed);
    }

    /// Adds the specified number of bytes to the sent bytes counter.
    ///
    /// This method is called whenever data is sent to a client.
    ///
    /// # Arguments
    ///
    /// * `bytes` - The number of bytes sent
    #[inline(always)]
    pub fn bytes_sent_add(&self, bytes: u64) {
        self.total.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        self.current.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Adds the specified time to the transaction time counter and records it in the histogram.
    ///
    /// This method records transaction times in an HDR histogram for efficient percentile
    /// calculations. Values exceeding the histogram maximum are clamped.
    ///
    /// # Arguments
    ///
    /// * `microseconds` - The transaction time in microseconds
    #[inline(always)]
    pub fn xact_time_add(&self, microseconds: u64) {
        // Skip recording zero transaction times
        if microseconds == 0 {
            return;
        }

        // Update total and current transaction time counters
        self.total
            .xact_time_microseconds
            .fetch_add(microseconds, Ordering::Relaxed);
        self.current
            .xact_time_microseconds
            .fetch_add(microseconds, Ordering::Relaxed);

        // Record the transaction time in the histogram if we can acquire the lock
        if let Some(mut histogram) = self.xact_histogram.try_lock() {
            // Clamp value to histogram max to avoid errors
            let value = microseconds.min(HISTOGRAM_MAX_VALUE_US);
            let _ = histogram.record(value);
        }
    }

    /// Adds the specified time to the query time counter and records it in the histogram.
    ///
    /// This method records query times in an HDR histogram for efficient percentile
    /// calculations. Values exceeding the histogram maximum are clamped.
    ///
    /// # Arguments
    ///
    /// * `microseconds` - The query time in microseconds
    #[inline(always)]
    pub fn query_time_add_microseconds(&self, microseconds: u64) {
        // Update total and current query time counters
        self.total
            .query_time_microseconds
            .fetch_add(microseconds, Ordering::Relaxed);
        self.current
            .query_time_microseconds
            .fetch_add(microseconds, Ordering::Relaxed);

        // Record the query time in the histogram if we can acquire the lock
        if let Some(mut histogram) = self.query_histogram.try_lock() {
            // Clamp value to histogram max to avoid errors
            let value = microseconds.min(HISTOGRAM_MAX_VALUE_US);
            let _ = histogram.record(value);
        }
    }

    /// Adds the specified time to the wait time counter.
    ///
    /// This method is called whenever a client waits for a resource.
    ///
    /// # Arguments
    ///
    /// * `time` - The wait time in microseconds
    #[inline(always)]
    pub fn wait_time_add(&self, time: u64) {
        self.total.wait_time.fetch_add(time, Ordering::Relaxed);
        self.current.wait_time.fetch_add(time, Ordering::Relaxed);
    }

    /// Increments the error counter in both total and current statistics.
    ///
    /// This method is called whenever an error occurs during query processing.
    #[inline(always)]
    pub fn error(&self) {
        self.total.errors.fetch_add(1, Ordering::Relaxed);
        self.current.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns transaction time percentiles (p50, p90, p95, p99) in microseconds.
    ///
    /// Uses HDR histogram for O(1) percentile calculation.
    pub fn get_xact_percentiles(&self) -> (u64, u64, u64, u64) {
        let histogram = self.xact_histogram.lock();
        (
            histogram.value_at_quantile(0.50),
            histogram.value_at_quantile(0.90),
            histogram.value_at_quantile(0.95),
            histogram.value_at_quantile(0.99),
        )
    }

    /// Returns query time percentiles (p50, p90, p95, p99) in microseconds.
    ///
    /// Uses HDR histogram for O(1) percentile calculation.
    pub fn get_query_percentiles(&self) -> (u64, u64, u64, u64) {
        let histogram = self.query_histogram.lock();
        (
            histogram.value_at_quantile(0.50),
            histogram.value_at_quantile(0.90),
            histogram.value_at_quantile(0.95),
            histogram.value_at_quantile(0.99),
        )
    }

    /// Resets the histograms for the new time window.
    ///
    /// Called at the end of each stats period (15 seconds) to start fresh.
    pub fn reset_histograms(&self) {
        if let Some(mut histogram) = self.xact_histogram.try_lock() {
            histogram.reset();
        }
        if let Some(mut histogram) = self.query_histogram.try_lock() {
            histogram.reset();
        }
    }

    /// Updates the average statistics based on the current period's values.
    ///
    /// This method calculates per-second averages for all metrics and average times per transaction/query.
    /// It is called periodically by the stats collector to update the reported averages.
    pub fn update_averages(&self) {
        // Convert the stat period from milliseconds to seconds for per-second calculations
        let stat_period_per_second = crate::stats::STAT_PERIOD / 1_000;

        // Calculate transaction-related averages
        self.update_transaction_averages(stat_period_per_second);

        // Calculate query-related averages
        self.update_query_averages(stat_period_per_second);

        // Calculate throughput averages (bytes received/sent)
        self.update_throughput_averages(stat_period_per_second);

        // Calculate wait time and error averages
        self.update_wait_and_error_averages(stat_period_per_second);
    }

    /// Helper method to update transaction-related averages
    fn update_transaction_averages(&self, stat_period_per_second: u64) {
        let current_xact_count = self.current.xact_count.load(Ordering::Relaxed);
        let current_xact_time = self.current.xact_time_microseconds.load(Ordering::Relaxed);

        // Calculate transactions per second
        self.averages.xact_count.store(
            current_xact_count / stat_period_per_second,
            Ordering::Relaxed,
        );

        // Calculate average time per transaction (or 0 if no transactions)
        if current_xact_count == 0 {
            self.averages
                .xact_time_microseconds
                .store(0, Ordering::Relaxed);
        } else {
            self.averages
                .xact_time_microseconds
                .store(current_xact_time / current_xact_count, Ordering::Relaxed);
        }
    }

    /// Helper method to update query-related averages
    fn update_query_averages(&self, stat_period_per_second: u64) {
        let current_query_count = self.current.query_count.load(Ordering::Relaxed);
        let current_query_time = self.current.query_time_microseconds.load(Ordering::Relaxed);

        // Calculate queries per second
        self.averages.query_count.store(
            current_query_count / stat_period_per_second,
            Ordering::Relaxed,
        );

        // Calculate average time per query (or 0 if no queries)
        if current_query_count == 0 {
            self.averages
                .query_time_microseconds
                .store(0, Ordering::Relaxed);
        } else {
            self.averages
                .query_time_microseconds
                .store(current_query_time / current_query_count, Ordering::Relaxed);
        }
    }

    /// Helper method to update throughput averages
    fn update_throughput_averages(&self, stat_period_per_second: u64) {
        // Calculate bytes received per second
        let current_bytes_received = self.current.bytes_received.load(Ordering::Relaxed);
        self.averages.bytes_received.store(
            current_bytes_received / stat_period_per_second,
            Ordering::Relaxed,
        );

        // Calculate bytes sent per second
        let current_bytes_sent = self.current.bytes_sent.load(Ordering::Relaxed);
        self.averages.bytes_sent.store(
            current_bytes_sent / stat_period_per_second,
            Ordering::Relaxed,
        );
    }

    /// Helper method to update wait time and error averages
    fn update_wait_and_error_averages(&self, stat_period_per_second: u64) {
        // Calculate average wait time per second
        let current_wait_time = self.current.wait_time.load(Ordering::Relaxed);
        self.averages.wait_time.store(
            current_wait_time / stat_period_per_second,
            Ordering::Relaxed,
        );

        // Calculate errors per second
        let current_errors = self.current.errors.load(Ordering::Relaxed);
        self.averages
            .errors
            .store(current_errors / stat_period_per_second, Ordering::Relaxed);
    }

    /// Resets all current period counters to zero.
    ///
    /// This method is called after the averages have been updated to prepare for the next period.
    pub fn reset_current_counts(&self) {
        // Reset transaction-related counters
        self.current.xact_count.store(0, Ordering::Relaxed);
        self.current
            .xact_time_microseconds
            .store(0, Ordering::Relaxed);

        // Reset query-related counters
        self.current.query_count.store(0, Ordering::Relaxed);
        self.current
            .query_time_microseconds
            .store(0, Ordering::Relaxed);

        // Reset throughput counters
        self.current.bytes_received.store(0, Ordering::Relaxed);
        self.current.bytes_sent.store(0, Ordering::Relaxed);

        // Reset wait time and error counters
        self.current.wait_time.store(0, Ordering::Relaxed);
        self.current.errors.store(0, Ordering::Relaxed);
    }

    /// Populates a row vector with string representations of all statistics.
    ///
    /// This method is used for generating reports or displaying statistics in a tabular format.
    ///
    /// # Arguments
    ///
    /// * `row` - A mutable reference to a vector of strings that will be populated with statistics
    pub fn populate_row(&self, row: &mut Vec<String>) {
        // Convert all statistics to strings and add them to the row
        for (_key, value) in self {
            row.push(value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_address_stat_fields_default() {
        // Test that AddressStatFields initializes with all zeros
        let fields = AddressStatFields::default();

        assert_eq!(fields.xact_count.load(Ordering::Relaxed), 0);
        assert_eq!(fields.query_count.load(Ordering::Relaxed), 0);
        assert_eq!(fields.bytes_received.load(Ordering::Relaxed), 0);
        assert_eq!(fields.bytes_sent.load(Ordering::Relaxed), 0);
        assert_eq!(fields.xact_time_microseconds.load(Ordering::Relaxed), 0);
        assert_eq!(fields.query_time_microseconds.load(Ordering::Relaxed), 0);
        assert_eq!(fields.wait_time.load(Ordering::Relaxed), 0);
        assert_eq!(fields.errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_address_stats_default() {
        // Test that AddressStats initializes with all zeros
        let stats = AddressStats::default();

        // Check total fields
        assert_eq!(stats.total.xact_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.total.query_count.load(Ordering::Relaxed), 0);

        // Check current fields
        assert_eq!(stats.current.xact_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.current.query_count.load(Ordering::Relaxed), 0);

        // Check averages fields
        assert_eq!(stats.averages.xact_count.load(Ordering::Relaxed), 0);
        assert_eq!(stats.averages.query_count.load(Ordering::Relaxed), 0);

        // Check other fields
        assert!(!stats.averages_updated.load(Ordering::Relaxed));
        // Check histograms are empty (len() == 0)
        assert_eq!(stats.xact_histogram.lock().len(), 0);
        assert_eq!(stats.query_histogram.lock().len(), 0);
    }

    #[test]
    fn test_counter_methods() {
        let stats = AddressStats::default();

        // Test xact_count_add
        stats.xact_count_add();
        assert_eq!(stats.total.xact_count.load(Ordering::Relaxed), 1);
        assert_eq!(stats.current.xact_count.load(Ordering::Relaxed), 1);

        // Test query_count_add
        stats.query_count_add();
        assert_eq!(stats.total.query_count.load(Ordering::Relaxed), 1);
        assert_eq!(stats.current.query_count.load(Ordering::Relaxed), 1);

        // Test bytes_received_add
        stats.bytes_received_add(100);
        assert_eq!(stats.total.bytes_received.load(Ordering::Relaxed), 100);
        assert_eq!(stats.current.bytes_received.load(Ordering::Relaxed), 100);

        // Test bytes_sent_add
        stats.bytes_sent_add(200);
        assert_eq!(stats.total.bytes_sent.load(Ordering::Relaxed), 200);
        assert_eq!(stats.current.bytes_sent.load(Ordering::Relaxed), 200);

        // Test wait_time_add
        stats.wait_time_add(300);
        assert_eq!(stats.total.wait_time.load(Ordering::Relaxed), 300);
        assert_eq!(stats.current.wait_time.load(Ordering::Relaxed), 300);

        // Test error
        stats.error();
        assert_eq!(stats.total.errors.load(Ordering::Relaxed), 1);
        assert_eq!(stats.current.errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_time_recording_methods() {
        let stats = AddressStats::default();

        // Test xact_time_add with non-zero value
        stats.xact_time_add(150);
        assert_eq!(
            stats.total.xact_time_microseconds.load(Ordering::Relaxed),
            150
        );
        assert_eq!(
            stats.current.xact_time_microseconds.load(Ordering::Relaxed),
            150
        );

        // Verify the time was recorded in the histogram
        {
            let histogram = stats.xact_histogram.lock();
            assert_eq!(histogram.len(), 1);
        }

        // Test xact_time_add with zero value (should be ignored)
        stats.xact_time_add(0);
        assert_eq!(
            stats.total.xact_time_microseconds.load(Ordering::Relaxed),
            150
        ); // Unchanged
        assert_eq!(
            stats.current.xact_time_microseconds.load(Ordering::Relaxed),
            150
        ); // Unchanged

        // Test query_time_add_microseconds
        stats.query_time_add_microseconds(250);
        assert_eq!(
            stats.total.query_time_microseconds.load(Ordering::Relaxed),
            250
        );
        assert_eq!(
            stats
                .current
                .query_time_microseconds
                .load(Ordering::Relaxed),
            250
        );

        // Verify the time was recorded in the histogram
        {
            let histogram = stats.query_histogram.lock();
            assert_eq!(histogram.len(), 1);
        }
    }

    #[test]
    fn test_histogram_percentiles() {
        let stats = AddressStats::default();

        // Add values to create a known distribution
        // Add 100 values: 1, 2, 3, ..., 100 microseconds
        for i in 1..=100 {
            stats.xact_time_add(i as u64);
            stats.query_time_add_microseconds(i as u64);
        }

        // Verify percentiles for transactions
        let (p50, p90, p95, p99) = stats.get_xact_percentiles();
        // p50 should be around 50, p90 around 90, p95 around 95, p99 around 99
        assert!(p50 >= 45 && p50 <= 55, "p50 xact should be ~50, got {}", p50);
        assert!(p90 >= 85 && p90 <= 95, "p90 xact should be ~90, got {}", p90);
        assert!(p95 >= 90 && p95 <= 100, "p95 xact should be ~95, got {}", p95);
        assert!(p99 >= 95 && p99 <= 105, "p99 xact should be ~99, got {}", p99);

        // Verify percentiles for queries
        let (p50, p90, p95, p99) = stats.get_query_percentiles();
        assert!(p50 >= 45 && p50 <= 55, "p50 query should be ~50, got {}", p50);
        assert!(p90 >= 85 && p90 <= 95, "p90 query should be ~90, got {}", p90);
        assert!(p95 >= 90 && p95 <= 100, "p95 query should be ~95, got {}", p95);
        assert!(p99 >= 95 && p99 <= 105, "p99 query should be ~99, got {}", p99);
    }

    #[test]
    fn test_histogram_reset() {
        let stats = AddressStats::default();

        // Add some values
        for i in 1..=10 {
            stats.xact_time_add(i as u64);
        }

        // Verify histogram has data
        assert_eq!(stats.xact_histogram.lock().len(), 10);

        // Reset histograms
        stats.reset_histograms();

        // Verify histogram is empty
        assert_eq!(stats.xact_histogram.lock().len(), 0);
    }

    #[test]
    fn test_update_averages_and_reset() {
        // We need to mock the STAT_PERIOD for testing
        // Since we can't modify the constant directly, we'll test with known values

        let stats = AddressStats::default();

        // Add some data
        stats.xact_count_add();
        stats.xact_count_add();
        stats.xact_time_add(1000); // 1000 microseconds for first transaction
        stats.xact_time_add(2000); // 2000 microseconds for second transaction

        stats.query_count_add();
        stats.query_count_add();
        stats.query_count_add();
        stats.query_time_add_microseconds(300); // 300 microseconds for first query
        stats.query_time_add_microseconds(400); // 400 microseconds for second query
        stats.query_time_add_microseconds(500); // 500 microseconds for third query

        stats.bytes_received_add(15000);
        stats.bytes_sent_add(25000);
        stats.wait_time_add(500);
        stats.error();
        stats.error();

        // Update averages (assuming STAT_PERIOD is 15000 milliseconds = 15 seconds)
        stats.update_averages();

        // Check averages (transactions per second = 2/15 = 0)
        assert_eq!(stats.averages.xact_count.load(Ordering::Relaxed), 0);
        // Average transaction time = (1000 + 2000) / 2 = 1500 microseconds
        assert_eq!(
            stats
                .averages
                .xact_time_microseconds
                .load(Ordering::Relaxed),
            1500
        );

        // Check averages (queries per second = 3/15 = 0)
        assert_eq!(stats.averages.query_count.load(Ordering::Relaxed), 0);
        // Average query time = (300 + 400 + 500) / 3 = 400 microseconds
        assert_eq!(
            stats
                .averages
                .query_time_microseconds
                .load(Ordering::Relaxed),
            400
        );

        // Check throughput averages (bytes per second)
        assert_eq!(
            stats.averages.bytes_received.load(Ordering::Relaxed),
            15000 / 15
        );
        assert_eq!(
            stats.averages.bytes_sent.load(Ordering::Relaxed),
            25000 / 15
        );

        // Check wait time and error averages
        assert_eq!(stats.averages.wait_time.load(Ordering::Relaxed), 500 / 15);
        assert_eq!(stats.averages.errors.load(Ordering::Relaxed), 2 / 15);

        // Now reset current counts
        stats.reset_current_counts();

        // Verify current counts are reset to zero
        assert_eq!(stats.current.xact_count.load(Ordering::Relaxed), 0);
        assert_eq!(
            stats.current.xact_time_microseconds.load(Ordering::Relaxed),
            0
        );
        assert_eq!(stats.current.query_count.load(Ordering::Relaxed), 0);
        assert_eq!(
            stats
                .current
                .query_time_microseconds
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(stats.current.bytes_received.load(Ordering::Relaxed), 0);
        assert_eq!(stats.current.bytes_sent.load(Ordering::Relaxed), 0);
        assert_eq!(stats.current.wait_time.load(Ordering::Relaxed), 0);
        assert_eq!(stats.current.errors.load(Ordering::Relaxed), 0);

        // But total counts should remain unchanged
        assert_eq!(stats.total.xact_count.load(Ordering::Relaxed), 2);
        assert_eq!(
            stats.total.xact_time_microseconds.load(Ordering::Relaxed),
            3000
        );
        assert_eq!(stats.total.query_count.load(Ordering::Relaxed), 3);
        assert_eq!(
            stats.total.query_time_microseconds.load(Ordering::Relaxed),
            1200
        );
    }

    #[test]
    fn test_into_iterator() {
        let stats = AddressStats::default();

        // Add some data
        stats.total.xact_count.store(10, Ordering::Relaxed);
        stats.total.query_count.store(20, Ordering::Relaxed);
        stats.total.bytes_received.store(1000, Ordering::Relaxed);
        stats.total.bytes_sent.store(2000, Ordering::Relaxed);
        stats
            .total
            .xact_time_microseconds
            .store(5000, Ordering::Relaxed);
        stats
            .total
            .query_time_microseconds
            .store(6000, Ordering::Relaxed);
        stats.total.wait_time.store(300, Ordering::Relaxed);
        stats.total.errors.store(5, Ordering::Relaxed);

        stats.averages.xact_count.store(2, Ordering::Relaxed);
        stats.averages.query_count.store(4, Ordering::Relaxed);
        stats.averages.bytes_received.store(200, Ordering::Relaxed);
        stats.averages.bytes_sent.store(400, Ordering::Relaxed);
        stats
            .averages
            .xact_time_microseconds
            .store(500, Ordering::Relaxed);
        stats
            .averages
            .query_time_microseconds
            .store(300, Ordering::Relaxed);
        stats.averages.wait_time.store(30, Ordering::Relaxed);
        stats.averages.errors.store(1, Ordering::Relaxed);

        // Convert to iterator and collect into a HashMap for easy lookup
        let stats_map: HashMap<String, f64> = (&stats).into_iter().collect();

        // Check total values
        assert_eq!(stats_map.get("total_xact_count"), Some(&10.0));
        assert_eq!(stats_map.get("total_query_count"), Some(&20.0));
        assert_eq!(stats_map.get("total_received"), Some(&1000.0));
        assert_eq!(stats_map.get("total_sent"), Some(&2000.0));
        assert_eq!(stats_map.get("total_xact_time"), Some(&5.0)); // Converted to milliseconds
        assert_eq!(stats_map.get("total_query_time"), Some(&6.0)); // Converted to milliseconds
        assert_eq!(stats_map.get("total_wait_time"), Some(&300.0));
        assert_eq!(stats_map.get("total_errors"), Some(&5.0));

        // Check average values
        assert_eq!(stats_map.get("avg_xact_count"), Some(&2.0));
        assert_eq!(stats_map.get("avg_query_count"), Some(&4.0));
        assert_eq!(stats_map.get("avg_recv"), Some(&200.0));
        assert_eq!(stats_map.get("avg_sent"), Some(&400.0));
        assert_eq!(stats_map.get("avg_xact_time"), Some(&500.0));
        assert_eq!(stats_map.get("avg_query_time"), Some(&300.0));
        assert_eq!(stats_map.get("avg_wait_time"), Some(&30.0));
        assert_eq!(stats_map.get("avg_errors"), Some(&1.0));
    }

    #[test]
    fn test_populate_row() {
        let stats = AddressStats::default();

        // Add some data
        stats.total.xact_count.store(10, Ordering::Relaxed);
        stats.total.query_count.store(20, Ordering::Relaxed);

        // Create a row vector
        let mut row = Vec::new();

        // Populate the row
        stats.populate_row(&mut row);

        // Check that the row has the expected number of elements
        assert_eq!(row.len(), 16); // 8 total stats + 8 average stats

        // Check that the first element is "10" (total_xact_count)
        assert_eq!(row[0], "10");

        // Check that the second element is "20" (total_query_count)
        assert_eq!(row[1], "20");
    }

    #[test]
    fn test_thread_safety() {
        let stats = Arc::new(AddressStats::default());
        let mut handles = vec![];

        // Spawn 10 threads, each incrementing counters
        for _ in 0..10 {
            let stats_clone = Arc::clone(&stats);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    stats_clone.xact_count_add();
                    stats_clone.query_count_add();
                    stats_clone.bytes_received_add(10);
                    stats_clone.bytes_sent_add(20);
                    stats_clone.xact_time_add(5);
                    stats_clone.query_time_add_microseconds(3);
                    stats_clone.wait_time_add(2);
                    stats_clone.error();

                    // Small sleep to increase chance of thread interleaving
                    thread::sleep(Duration::from_micros(1));
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Check that all operations were counted correctly
        assert_eq!(stats.total.xact_count.load(Ordering::Relaxed), 1000); // 10 threads * 100 increments
        assert_eq!(stats.total.query_count.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.total.bytes_received.load(Ordering::Relaxed), 10000); // 10 threads * 100 * 10 bytes
        assert_eq!(stats.total.bytes_sent.load(Ordering::Relaxed), 20000); // 10 threads * 100 * 20 bytes
        assert_eq!(
            stats.total.xact_time_microseconds.load(Ordering::Relaxed),
            5000
        ); // 10 threads * 100 * 5 microseconds
        assert_eq!(
            stats.total.query_time_microseconds.load(Ordering::Relaxed),
            3000
        ); // 10 threads * 100 * 3 microseconds
        assert_eq!(stats.total.wait_time.load(Ordering::Relaxed), 2000); // 10 threads * 100 * 2 microseconds
        assert_eq!(stats.total.errors.load(Ordering::Relaxed), 1000); // 10 threads * 100 errors

        // Same checks for current counters
        assert_eq!(stats.current.xact_count.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.current.query_count.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.current.bytes_received.load(Ordering::Relaxed), 10000);
        assert_eq!(stats.current.bytes_sent.load(Ordering::Relaxed), 20000);
        assert_eq!(
            stats.current.xact_time_microseconds.load(Ordering::Relaxed),
            5000
        );
        assert_eq!(
            stats
                .current
                .query_time_microseconds
                .load(Ordering::Relaxed),
            3000
        );
        assert_eq!(stats.current.wait_time.load(Ordering::Relaxed), 2000);
        assert_eq!(stats.current.errors.load(Ordering::Relaxed), 1000);
    }
}
