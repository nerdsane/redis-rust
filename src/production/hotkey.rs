use std::collections::HashMap;

/// Per-key access metrics for hot key detection
#[derive(Clone, Debug)]
pub struct AccessMetrics {
    /// Number of read operations
    pub read_count: u64,
    /// Number of write operations
    pub write_count: u64,
    /// First access timestamp (milliseconds)
    pub first_access_ms: u64,
    /// Last access timestamp (milliseconds)
    pub last_access_ms: u64,
}

impl AccessMetrics {
    fn new(now_ms: u64, is_write: bool) -> Self {
        AccessMetrics {
            read_count: if is_write { 0 } else { 1 },
            write_count: if is_write { 1 } else { 0 },
            first_access_ms: now_ms,
            last_access_ms: now_ms,
        }
    }

    fn record(&mut self, now_ms: u64, is_write: bool) {
        if is_write {
            self.write_count = self.write_count.saturating_add(1);
        } else {
            self.read_count = self.read_count.saturating_add(1);
        }
        self.last_access_ms = now_ms;
    }

    /// Calculate access rate (ops/second) within the time window
    pub fn access_rate(&self, now_ms: u64) -> f64 {
        let total = self.read_count.saturating_add(self.write_count);
        let duration_ms = now_ms.saturating_sub(self.first_access_ms).max(1);
        (total as f64 * 1000.0) / duration_ms as f64
    }
}

/// Configuration for hot key detection
#[derive(Clone, Debug)]
pub struct HotKeyConfig {
    /// Sliding window size in milliseconds (default: 10 seconds)
    pub window_ms: u64,
    /// Threshold for "hot" classification (accesses per second, default: 100)
    pub hot_threshold: f64,
    /// How often to cleanup stale entries (default: 5 seconds)
    pub cleanup_interval_ms: u64,
    /// Maximum keys to track (to bound memory, default: 10000)
    pub max_tracked_keys: usize,
}

impl Default for HotKeyConfig {
    fn default() -> Self {
        HotKeyConfig {
            window_ms: 10_000,
            hot_threshold: 100.0,
            cleanup_interval_ms: 5_000,
            max_tracked_keys: 10_000,
        }
    }
}

/// Sliding window hot key detector
///
/// Tracks access frequency per key to identify "hot" keys that receive
/// disproportionate load. Hot keys can then be replicated more aggressively
/// or handled specially (e.g., caching, routing optimization).
pub struct HotKeyDetector {
    /// Access counts per key
    access_counts: HashMap<String, AccessMetrics>,
    /// Configuration
    config: HotKeyConfig,
    /// Last cleanup timestamp
    last_cleanup_ms: u64,
}

impl HotKeyDetector {
    /// Verify all invariants hold for this detector
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: access_counts.len() <= max_tracked_keys
        debug_assert!(
            self.access_counts.len() <= self.config.max_tracked_keys,
            "Invariant violated: tracked {} keys but max is {}",
            self.access_counts.len(),
            self.config.max_tracked_keys
        );

        // Invariant 2: All access metrics must have consistent timestamps
        for (key, metrics) in &self.access_counts {
            debug_assert!(
                metrics.first_access_ms <= metrics.last_access_ms,
                "Invariant violated: key '{}' has first_access_ms > last_access_ms",
                key
            );

            // Invariant 3: Total accesses must be > 0 for tracked keys
            let total = metrics.read_count + metrics.write_count;
            debug_assert!(
                total > 0,
                "Invariant violated: key '{}' has zero total accesses",
                key
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

    pub fn new(config: HotKeyConfig) -> Self {
        HotKeyDetector {
            access_counts: HashMap::with_capacity(1024),
            config,
            last_cleanup_ms: 0,
        }
    }

    /// Record an access to a key
    #[inline]
    pub fn record_access(&mut self, key: &str, is_write: bool, now_ms: u64) {
        // Periodic cleanup to bound memory
        if now_ms.saturating_sub(self.last_cleanup_ms) >= self.config.cleanup_interval_ms {
            self.cleanup_stale(now_ms);
            self.last_cleanup_ms = now_ms;
        }

        if let Some(metrics) = self.access_counts.get_mut(key) {
            metrics.record(now_ms, is_write);
        } else {
            // Only add new key if under limit
            if self.access_counts.len() < self.config.max_tracked_keys {
                self.access_counts
                    .insert(key.to_string(), AccessMetrics::new(now_ms, is_write));
            }
        }
    }

    /// Check if a specific key is currently hot
    pub fn is_hot(&self, key: &str, now_ms: u64) -> bool {
        self.access_counts
            .get(key)
            .map(|m| m.access_rate(now_ms) >= self.config.hot_threshold)
            .unwrap_or(false)
    }

    /// Get all currently hot keys with their access rates
    pub fn get_hot_keys(&self, now_ms: u64) -> Vec<(String, f64)> {
        self.access_counts
            .iter()
            .filter_map(|(key, metrics)| {
                let rate = metrics.access_rate(now_ms);
                if rate >= self.config.hot_threshold {
                    Some((key.clone(), rate))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get the top N hottest keys by access rate
    pub fn get_top_keys(&self, n: usize, now_ms: u64) -> Vec<(String, f64)> {
        let mut rates: Vec<_> = self
            .access_counts
            .iter()
            .map(|(key, metrics)| (key.clone(), metrics.access_rate(now_ms)))
            .collect();

        rates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rates.truncate(n);
        rates
    }

    /// Get access metrics for a specific key
    pub fn get_metrics(&self, key: &str) -> Option<&AccessMetrics> {
        self.access_counts.get(key)
    }

    /// Remove stale entries outside the sliding window
    pub fn cleanup_stale(&mut self, now_ms: u64) {
        let window_start = now_ms.saturating_sub(self.config.window_ms);
        self.access_counts
            .retain(|_, metrics| metrics.last_access_ms >= window_start);
    }

    /// Get the number of currently tracked keys
    pub fn tracked_key_count(&self) -> usize {
        self.access_counts.len()
    }

    /// Clear all tracking data
    pub fn clear(&mut self) {
        self.access_counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hot_key_detection() {
        let config = HotKeyConfig {
            window_ms: 1000,
            hot_threshold: 10.0, // 10 ops/sec for testing
            cleanup_interval_ms: 500,
            max_tracked_keys: 100,
        };
        let mut detector = HotKeyDetector::new(config);

        // Simulate 20 accesses in 1 second (20 ops/sec)
        for i in 0..20 {
            detector.record_access("hot_key", false, i * 50);
        }
        // Simulate 5 accesses in 1 second (5 ops/sec)
        for i in 0..5 {
            detector.record_access("cold_key", false, i * 200);
        }

        let now = 1000;
        assert!(detector.is_hot("hot_key", now));
        assert!(!detector.is_hot("cold_key", now));

        let hot_keys = detector.get_hot_keys(now);
        assert_eq!(hot_keys.len(), 1);
        assert_eq!(hot_keys[0].0, "hot_key");
    }

    #[test]
    fn test_access_metrics() {
        let mut metrics = AccessMetrics::new(0, false);
        assert_eq!(metrics.read_count, 1);
        assert_eq!(metrics.write_count, 0);

        metrics.record(100, true);
        assert_eq!(metrics.read_count, 1);
        assert_eq!(metrics.write_count, 1);

        // 2 ops over 100ms = 20 ops/sec
        let rate = metrics.access_rate(100);
        assert!((rate - 20.0).abs() < 0.1);
    }

    #[test]
    fn test_cleanup_stale() {
        let config = HotKeyConfig {
            window_ms: 100,
            hot_threshold: 10.0,
            cleanup_interval_ms: 50,
            max_tracked_keys: 100,
        };
        let mut detector = HotKeyDetector::new(config);

        detector.record_access("key1", false, 0);
        detector.record_access("key2", false, 50);
        assert_eq!(detector.tracked_key_count(), 2);

        // After window expires for key1
        detector.cleanup_stale(150);
        assert_eq!(detector.tracked_key_count(), 1);
        assert!(detector.access_counts.contains_key("key2"));
        assert!(!detector.access_counts.contains_key("key1"));
    }

    #[test]
    fn test_top_keys() {
        let config = HotKeyConfig::default();
        let mut detector = HotKeyDetector::new(config);

        // Different access counts
        for _ in 0..100 {
            detector.record_access("hot", false, 0);
        }
        for _ in 0..50 {
            detector.record_access("warm", false, 0);
        }
        for _ in 0..10 {
            detector.record_access("cold", false, 0);
        }

        let top = detector.get_top_keys(2, 1000);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "hot");
        assert_eq!(top[1].0, "warm");
    }

    #[test]
    fn test_max_tracked_keys() {
        let config = HotKeyConfig {
            max_tracked_keys: 3,
            ..Default::default()
        };
        let mut detector = HotKeyDetector::new(config);

        detector.record_access("key1", false, 0);
        detector.record_access("key2", false, 0);
        detector.record_access("key3", false, 0);
        detector.record_access("key4", false, 0); // Should be ignored

        assert_eq!(detector.tracked_key_count(), 3);
        assert!(!detector.access_counts.contains_key("key4"));
    }
}
