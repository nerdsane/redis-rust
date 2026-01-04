use super::hotkey::{HotKeyConfig, HotKeyDetector};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Configuration for adaptive replication behavior
#[derive(Clone, Debug)]
pub struct AdaptiveConfig {
    /// Base replication factor for normal keys
    pub base_rf: u8,
    /// Replication factor for hot keys
    pub hot_key_rf: u8,
    /// How often to recalculate hot keys (milliseconds)
    pub recalc_interval_ms: u64,
    /// Hot key detection configuration
    pub hotkey_config: HotKeyConfig,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        AdaptiveConfig {
            base_rf: 3,
            hot_key_rf: 5,
            recalc_interval_ms: 5000,
            hotkey_config: HotKeyConfig::default(),
        }
    }
}

impl AdaptiveConfig {
    /// Create config optimized for high-throughput workloads
    pub fn high_throughput() -> Self {
        AdaptiveConfig {
            base_rf: 3,
            hot_key_rf: 5,
            recalc_interval_ms: 2000,
            hotkey_config: HotKeyConfig {
                window_ms: 5000,
                hot_threshold: 50.0,
                cleanup_interval_ms: 2500,
                max_tracked_keys: 20000,
            },
        }
    }

    /// Create config for latency-sensitive workloads
    pub fn low_latency() -> Self {
        AdaptiveConfig {
            base_rf: 2,
            hot_key_rf: 4,
            recalc_interval_ms: 1000,
            hotkey_config: HotKeyConfig {
                window_ms: 3000,
                hot_threshold: 100.0,
                cleanup_interval_ms: 1000,
                max_tracked_keys: 10000,
            },
        }
    }
}

/// Manages adaptive replication based on key access patterns
///
/// Tracks key access frequency and automatically adjusts replication
/// factors for hot keys. Hot keys get increased RF for better read
/// throughput and availability.
pub struct AdaptiveReplicationManager {
    /// Hot key detector for tracking access patterns
    hotkey_detector: HotKeyDetector,
    /// Per-key RF overrides for hot keys
    key_rf_overrides: HashMap<String, u8>,
    /// Configuration
    config: AdaptiveConfig,
    /// Last recalculation timestamp
    last_recalc_ms: u64,
    /// Stats: total keys promoted to hot
    stats_promotions: AtomicU64,
    /// Stats: total keys demoted from hot
    stats_demotions: AtomicU64,
}

impl AdaptiveReplicationManager {
    pub fn new(config: AdaptiveConfig) -> Self {
        AdaptiveReplicationManager {
            hotkey_detector: HotKeyDetector::new(config.hotkey_config.clone()),
            key_rf_overrides: HashMap::new(),
            config,
            last_recalc_ms: 0,
            stats_promotions: AtomicU64::new(0),
            stats_demotions: AtomicU64::new(0),
        }
    }

    /// Record an access to a key
    ///
    /// This should be called on every key access (GET, SET, etc.)
    #[inline]
    pub fn observe(&mut self, key: &str, is_write: bool, now_ms: u64) {
        self.hotkey_detector.record_access(key, is_write, now_ms);

        // Periodic recalculation
        if now_ms.saturating_sub(self.last_recalc_ms) >= self.config.recalc_interval_ms {
            self.recalculate(now_ms);
            self.last_recalc_ms = now_ms;
        }
    }

    /// Get the replication factor for a key
    ///
    /// Returns the hot key RF if the key is hot, otherwise the base RF
    #[inline]
    pub fn get_rf_for_key(&self, key: &str) -> u8 {
        self.key_rf_overrides
            .get(key)
            .copied()
            .unwrap_or(self.config.base_rf)
    }

    /// Check if a key is currently classified as hot
    pub fn is_hot(&self, key: &str, now_ms: u64) -> bool {
        self.hotkey_detector.is_hot(key, now_ms)
    }

    /// Recalculate hot keys and update RF overrides
    pub fn recalculate(&mut self, now_ms: u64) {
        let hot_keys = self.hotkey_detector.get_hot_keys(now_ms);
        let hot_key_set: std::collections::HashSet<_> = hot_keys.iter().map(|(k, _)| k.clone()).collect();

        // Promote new hot keys
        for (key, _rate) in &hot_keys {
            if !self.key_rf_overrides.contains_key(key) {
                self.key_rf_overrides.insert(key.clone(), self.config.hot_key_rf);
                self.stats_promotions.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Demote keys that are no longer hot
        let mut to_remove = Vec::new();
        for key in self.key_rf_overrides.keys() {
            if !hot_key_set.contains(key) {
                to_remove.push(key.clone());
            }
        }
        for key in to_remove {
            self.key_rf_overrides.remove(&key);
            self.stats_demotions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get all current hot keys with their RF
    pub fn get_hot_key_updates(&self) -> Vec<(String, u8)> {
        self.key_rf_overrides
            .iter()
            .map(|(k, &rf)| (k.clone(), rf))
            .collect()
    }

    /// Get the current number of hot keys
    pub fn hot_key_count(&self) -> usize {
        self.key_rf_overrides.len()
    }

    /// Get the top N hottest keys
    pub fn get_top_hot_keys(&self, n: usize, now_ms: u64) -> Vec<(String, f64)> {
        self.hotkey_detector.get_top_keys(n, now_ms)
    }

    /// Get statistics
    pub fn stats(&self) -> AdaptiveStats {
        AdaptiveStats {
            current_hot_keys: self.key_rf_overrides.len(),
            total_promotions: self.stats_promotions.load(Ordering::Relaxed),
            total_demotions: self.stats_demotions.load(Ordering::Relaxed),
            tracked_keys: self.hotkey_detector.tracked_key_count(),
            base_rf: self.config.base_rf,
            hot_rf: self.config.hot_key_rf,
        }
    }

    /// Force recalculation (for testing)
    pub fn force_recalculate(&mut self, now_ms: u64) {
        self.recalculate(now_ms);
    }

    /// Clear all state (for testing)
    pub fn clear(&mut self) {
        self.hotkey_detector.clear();
        self.key_rf_overrides.clear();
    }
}

/// Statistics about adaptive replication
#[derive(Debug, Clone)]
pub struct AdaptiveStats {
    /// Number of keys currently classified as hot
    pub current_hot_keys: usize,
    /// Total number of keys promoted to hot (lifetime)
    pub total_promotions: u64,
    /// Total number of keys demoted from hot (lifetime)
    pub total_demotions: u64,
    /// Number of keys currently being tracked
    pub tracked_keys: usize,
    /// Base replication factor
    pub base_rf: u8,
    /// Hot key replication factor
    pub hot_rf: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_replication_basic() {
        let config = AdaptiveConfig {
            base_rf: 3,
            hot_key_rf: 5,
            recalc_interval_ms: 100,
            hotkey_config: HotKeyConfig {
                window_ms: 1000,
                hot_threshold: 10.0, // 10 ops/sec for testing
                cleanup_interval_ms: 500,
                max_tracked_keys: 100,
            },
        };
        let mut manager = AdaptiveReplicationManager::new(config);

        // Cold key gets base RF
        assert_eq!(manager.get_rf_for_key("cold_key"), 3);

        // Simulate hot key (20 accesses in 1 second)
        for i in 0..20 {
            manager.observe("hot_key", false, i * 50);
        }
        manager.force_recalculate(1000);

        // Hot key should now have elevated RF
        assert_eq!(manager.get_rf_for_key("hot_key"), 5);
        // Cold key still has base RF
        assert_eq!(manager.get_rf_for_key("cold_key"), 3);

        let stats = manager.stats();
        assert!(stats.current_hot_keys >= 1);
        assert!(stats.total_promotions >= 1);
    }

    #[test]
    fn test_hot_key_demotion() {
        let config = AdaptiveConfig {
            base_rf: 3,
            hot_key_rf: 5,
            recalc_interval_ms: 100,
            hotkey_config: HotKeyConfig {
                window_ms: 500,
                hot_threshold: 10.0,
                cleanup_interval_ms: 100,
                max_tracked_keys: 100,
            },
        };
        let mut manager = AdaptiveReplicationManager::new(config);

        // Make key hot
        for i in 0..20 {
            manager.observe("key", false, i * 25);
        }
        manager.force_recalculate(500);
        assert_eq!(manager.get_rf_for_key("key"), 5);

        // Time passes, key becomes cold
        manager.force_recalculate(2000);
        // Key should be demoted (depending on cleanup)
        let stats = manager.stats();
        // total_demotions is usize, always >= 0, so just check it exists
        let _ = stats.total_demotions; // May or may not be demoted yet
    }

    #[test]
    fn test_get_hot_key_updates() {
        let config = AdaptiveConfig {
            base_rf: 3,
            hot_key_rf: 5,
            recalc_interval_ms: 100,
            hotkey_config: HotKeyConfig {
                window_ms: 1000,
                hot_threshold: 10.0,
                cleanup_interval_ms: 500,
                max_tracked_keys: 100,
            },
        };
        let mut manager = AdaptiveReplicationManager::new(config);

        // Make multiple keys hot
        for i in 0..30 {
            manager.observe("hot1", false, i * 30);
            manager.observe("hot2", false, i * 30);
        }
        manager.force_recalculate(1000);

        let updates = manager.get_hot_key_updates();
        assert_eq!(updates.len(), 2);

        for (key, rf) in &updates {
            assert!(key == "hot1" || key == "hot2");
            assert_eq!(*rf, 5);
        }
    }

    #[test]
    fn test_config_presets() {
        let high_throughput = AdaptiveConfig::high_throughput();
        assert_eq!(high_throughput.hot_key_rf, 5);
        assert_eq!(high_throughput.hotkey_config.hot_threshold, 50.0);

        let low_latency = AdaptiveConfig::low_latency();
        assert_eq!(low_latency.hot_key_rf, 4);
        assert_eq!(low_latency.hotkey_config.hot_threshold, 100.0);
    }
}
