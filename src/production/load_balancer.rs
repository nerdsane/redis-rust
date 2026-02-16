use std::collections::HashMap;

/// Per-shard metrics for load balancing decisions
#[derive(Debug, Clone, Default)]
pub struct ShardMetrics {
    /// Shard identifier
    pub shard_id: usize,
    /// Estimated number of keys in this shard
    pub key_count: usize,
    /// Operations per second (rolling average)
    pub ops_per_second: f64,
    /// Estimated memory usage in bytes
    pub memory_bytes: usize,
    /// Last update timestamp (milliseconds)
    pub last_update_ms: u64,
}

impl ShardMetrics {
    pub fn new(shard_id: usize) -> Self {
        ShardMetrics {
            shard_id,
            ..Default::default()
        }
    }

    /// Calculate load score (higher = more loaded)
    pub fn load_score(&self) -> f64 {
        // Weight ops/sec more heavily than key count
        self.ops_per_second + (self.key_count as f64 * 0.01)
    }
}

/// Configuration for load balancing behavior
#[derive(Clone, Debug)]
pub struct LoadBalancerConfig {
    /// Maximum load imbalance ratio before rebalancing (default: 2.0)
    /// If max_load / min_load > this value, trigger rebalance
    pub max_imbalance: f64,
    /// Minimum keys to justify a shard split (default: 10000)
    pub min_keys_for_split: usize,
    /// Minimum ops/sec to justify adding a shard (default: 10000)
    pub min_ops_for_scale: f64,
    /// Cooldown between scaling decisions (milliseconds)
    pub scaling_cooldown_ms: u64,
    /// Minimum shards (never scale below this)
    pub min_shards: usize,
    /// Maximum shards (never scale above this)
    pub max_shards: usize,
}

impl Default for LoadBalancerConfig {
    fn default() -> Self {
        LoadBalancerConfig {
            max_imbalance: 2.0,
            min_keys_for_split: 10000,
            min_ops_for_scale: 10000.0,
            scaling_cooldown_ms: 30000, // 30 seconds
            min_shards: 1,
            max_shards: 256,
        }
    }
}

/// Scaling decisions returned by the load balancer
#[derive(Debug, Clone, PartialEq)]
pub enum ScalingDecision {
    /// No scaling needed
    NoChange,
    /// Add a new shard to handle increased load
    AddShard { reason: String, current_load: f64 },
    /// Remove an underutilized shard
    RemoveShard { shard_id: usize, reason: String },
    /// Rebalance keys between shards (informational)
    RebalanceRecommended {
        from_shard: usize,
        to_shard: usize,
        imbalance_ratio: f64,
    },
}

/// Manages load balancing and shard scaling decisions
///
/// Monitors per-shard metrics and recommends scaling actions
/// based on configurable thresholds.
pub struct ShardLoadBalancer {
    /// Per-shard metrics
    shard_metrics: HashMap<usize, ShardMetrics>,
    /// Configuration
    config: LoadBalancerConfig,
    /// Current number of shards
    num_shards: usize,
    /// Last scaling decision timestamp
    last_scaling_ms: u64,
    /// Rolling averages for smoothing
    ops_history: HashMap<usize, Vec<f64>>,
}

impl ShardLoadBalancer {
    /// Verify all invariants hold for this load balancer
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: min_shards <= num_shards <= max_shards
        debug_assert!(
            self.num_shards >= self.config.min_shards,
            "Invariant violated: num_shards {} < min_shards {}",
            self.num_shards,
            self.config.min_shards
        );
        debug_assert!(
            self.num_shards <= self.config.max_shards,
            "Invariant violated: num_shards {} > max_shards {}",
            self.num_shards,
            self.config.max_shards
        );

        // Invariant 2: shard_metrics should have entries for all shards
        debug_assert_eq!(
            self.shard_metrics.len(),
            self.num_shards,
            "Invariant violated: shard_metrics.len() {} != num_shards {}",
            self.shard_metrics.len(),
            self.num_shards
        );

        // Invariant 3: Each shard metric should have matching shard_id
        for (&id, metrics) in &self.shard_metrics {
            debug_assert_eq!(
                id, metrics.shard_id,
                "Invariant violated: shard_metrics key {} != metrics.shard_id {}",
                id, metrics.shard_id
            );
        }

        // Invariant 4: ops_per_second should be non-negative
        for (&id, metrics) in &self.shard_metrics {
            debug_assert!(
                metrics.ops_per_second >= 0.0,
                "Invariant violated: shard {} has negative ops_per_second {}",
                id,
                metrics.ops_per_second
            );
        }

        // Invariant 5: config consistency (min <= max)
        debug_assert!(
            self.config.min_shards <= self.config.max_shards,
            "Invariant violated: min_shards {} > max_shards {}",
            self.config.min_shards,
            self.config.max_shards
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

    pub fn new(num_shards: usize, config: LoadBalancerConfig) -> Self {
        let mut shard_metrics = HashMap::new();
        for i in 0..num_shards {
            shard_metrics.insert(i, ShardMetrics::new(i));
        }

        ShardLoadBalancer {
            shard_metrics,
            config,
            num_shards,
            last_scaling_ms: 0,
            ops_history: HashMap::new(),
        }
    }

    /// Update metrics for a specific shard
    pub fn update_metrics(&mut self, shard_id: usize, metrics: ShardMetrics) {
        // Track ops history for smoothing
        let history = self.ops_history.entry(shard_id).or_insert_with(Vec::new);
        history.push(metrics.ops_per_second);
        if history.len() > 10 {
            history.remove(0);
        }

        self.shard_metrics.insert(shard_id, metrics);
    }

    /// Record operations for a shard (incremental update)
    pub fn record_ops(&mut self, shard_id: usize, ops: u64, now_ms: u64) {
        if let Some(metrics) = self.shard_metrics.get_mut(&shard_id) {
            let elapsed_ms = now_ms.saturating_sub(metrics.last_update_ms).max(1);
            let ops_per_sec = (ops as f64 * 1000.0) / elapsed_ms as f64;

            // Exponential moving average for smoothing
            metrics.ops_per_second = metrics.ops_per_second * 0.8 + ops_per_sec * 0.2;
            metrics.last_update_ms = now_ms;
        }
    }

    /// Analyze current load and return scaling decision
    pub fn analyze(&self, now_ms: u64) -> ScalingDecision {
        // Check cooldown
        if now_ms.saturating_sub(self.last_scaling_ms) < self.config.scaling_cooldown_ms {
            return ScalingDecision::NoChange;
        }

        if self.shard_metrics.is_empty() {
            return ScalingDecision::NoChange;
        }

        // Calculate load statistics
        let loads: Vec<(usize, f64)> = self
            .shard_metrics
            .iter()
            .map(|(&id, m)| (id, m.load_score()))
            .collect();

        let total_ops: f64 = self.shard_metrics.values().map(|m| m.ops_per_second).sum();
        let total_keys: usize = self.shard_metrics.values().map(|m| m.key_count).sum();

        let min_load = loads.iter().map(|(_, l)| *l).fold(f64::MAX, f64::min);
        let max_load = loads.iter().map(|(_, l)| *l).fold(f64::MIN, f64::max);

        // Find most and least loaded shards
        // TigerStyle: Use unwrap_or for NaN safety instead of unwrap
        let most_loaded = loads
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let least_loaded = loads
            .iter()
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Check for imbalance
        if min_load > 0.0 && max_load / min_load > self.config.max_imbalance {
            if let (Some(&(from, _)), Some(&(to, _))) = (most_loaded, least_loaded) {
                return ScalingDecision::RebalanceRecommended {
                    from_shard: from,
                    to_shard: to,
                    imbalance_ratio: max_load / min_load,
                };
            }
        }

        // Check if we need more shards
        if self.num_shards < self.config.max_shards {
            let avg_ops = total_ops / self.num_shards as f64;
            if avg_ops > self.config.min_ops_for_scale
                || total_keys > self.config.min_keys_for_split * self.num_shards
            {
                return ScalingDecision::AddShard {
                    reason: format!("High load: {:.0} ops/sec avg, {} keys", avg_ops, total_keys),
                    current_load: total_ops,
                };
            }
        }

        // Check if we can remove a shard
        if self.num_shards > self.config.min_shards {
            if let Some(&(shard_id, load)) = least_loaded {
                // If least loaded shard has very little activity and we have many shards
                if load < 10.0 && self.num_shards > 4 {
                    return ScalingDecision::RemoveShard {
                        shard_id,
                        reason: format!("Underutilized: load score {:.2}", load),
                    };
                }
            }
        }

        ScalingDecision::NoChange
    }

    /// Mark that a scaling action was taken
    pub fn scaling_performed(&mut self, now_ms: u64) {
        self.last_scaling_ms = now_ms;
    }

    /// Add a new shard to tracking
    pub fn add_shard(&mut self, shard_id: usize) {
        self.shard_metrics
            .insert(shard_id, ShardMetrics::new(shard_id));
        self.num_shards += 1;
    }

    /// Remove a shard from tracking
    pub fn remove_shard(&mut self, shard_id: usize) {
        self.shard_metrics.remove(&shard_id);
        self.ops_history.remove(&shard_id);
        self.num_shards = self.num_shards.saturating_sub(1);
    }

    /// Get current load distribution as percentages
    pub fn get_distribution(&self) -> Vec<(usize, f64)> {
        let total_load: f64 = self.shard_metrics.values().map(|m| m.load_score()).sum();
        if total_load == 0.0 {
            return self
                .shard_metrics
                .keys()
                .map(|&id| (id, 100.0 / self.num_shards as f64))
                .collect();
        }

        self.shard_metrics
            .iter()
            .map(|(&id, m)| (id, (m.load_score() / total_load) * 100.0))
            .collect()
    }

    /// Get aggregate statistics
    pub fn get_stats(&self) -> LoadBalancerStats {
        let total_ops: f64 = self.shard_metrics.values().map(|m| m.ops_per_second).sum();
        let total_keys: usize = self.shard_metrics.values().map(|m| m.key_count).sum();
        let loads: Vec<f64> = self
            .shard_metrics
            .values()
            .map(|m| m.load_score())
            .collect();

        let min_load = loads.iter().cloned().fold(f64::MAX, f64::min);
        let max_load = loads.iter().cloned().fold(f64::MIN, f64::max);
        let avg_load = if loads.is_empty() {
            0.0
        } else {
            loads.iter().sum::<f64>() / loads.len() as f64
        };

        LoadBalancerStats {
            num_shards: self.num_shards,
            total_ops_per_sec: total_ops,
            total_keys,
            min_load,
            max_load,
            avg_load,
            imbalance_ratio: if min_load > 0.0 {
                max_load / min_load
            } else {
                1.0
            },
        }
    }

    /// Get metrics for a specific shard
    pub fn get_shard_metrics(&self, shard_id: usize) -> Option<&ShardMetrics> {
        self.shard_metrics.get(&shard_id)
    }
}

/// Aggregate statistics about load distribution
#[derive(Debug, Clone)]
pub struct LoadBalancerStats {
    pub num_shards: usize,
    pub total_ops_per_sec: f64,
    pub total_keys: usize,
    pub min_load: f64,
    pub max_load: f64,
    pub avg_load: f64,
    pub imbalance_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_balancer_basic() {
        let config = LoadBalancerConfig::default();
        let balancer = ShardLoadBalancer::new(4, config);

        assert_eq!(balancer.num_shards, 4);
        let stats = balancer.get_stats();
        assert_eq!(stats.num_shards, 4);
    }

    #[test]
    fn test_balanced_load_no_action() {
        let config = LoadBalancerConfig {
            scaling_cooldown_ms: 0,
            ..Default::default()
        };
        let mut balancer = ShardLoadBalancer::new(4, config);

        // Equal load across all shards
        for i in 0..4 {
            balancer.update_metrics(
                i,
                ShardMetrics {
                    shard_id: i,
                    key_count: 1000,
                    ops_per_second: 1000.0,
                    ..Default::default()
                },
            );
        }

        let decision = balancer.analyze(1000);
        assert_eq!(decision, ScalingDecision::NoChange);
    }

    #[test]
    fn test_imbalance_detection() {
        let config = LoadBalancerConfig {
            max_imbalance: 2.0,
            scaling_cooldown_ms: 0,
            ..Default::default()
        };
        let mut balancer = ShardLoadBalancer::new(4, config);

        // Create imbalance: shard 0 has 10x the load
        balancer.update_metrics(
            0,
            ShardMetrics {
                shard_id: 0,
                ops_per_second: 10000.0,
                ..Default::default()
            },
        );
        for i in 1..4 {
            balancer.update_metrics(
                i,
                ShardMetrics {
                    shard_id: i,
                    ops_per_second: 1000.0,
                    ..Default::default()
                },
            );
        }

        let decision = balancer.analyze(1000);
        match decision {
            ScalingDecision::RebalanceRecommended {
                from_shard,
                imbalance_ratio,
                ..
            } => {
                assert_eq!(from_shard, 0);
                assert!(imbalance_ratio > 2.0);
            }
            _ => panic!("Expected RebalanceRecommended, got {:?}", decision),
        }
    }

    #[test]
    fn test_scale_up_high_load() {
        let config = LoadBalancerConfig {
            min_ops_for_scale: 5000.0,
            scaling_cooldown_ms: 0,
            max_shards: 16,
            ..Default::default()
        };
        let mut balancer = ShardLoadBalancer::new(2, config);

        // High load that should trigger scale up
        balancer.update_metrics(
            0,
            ShardMetrics {
                shard_id: 0,
                ops_per_second: 20000.0,
                ..Default::default()
            },
        );
        balancer.update_metrics(
            1,
            ShardMetrics {
                shard_id: 1,
                ops_per_second: 20000.0,
                ..Default::default()
            },
        );

        let decision = balancer.analyze(1000);
        match decision {
            ScalingDecision::AddShard { .. } => {}
            _ => panic!("Expected AddShard, got {:?}", decision),
        }
    }

    #[test]
    fn test_cooldown_prevents_rapid_scaling() {
        let config = LoadBalancerConfig {
            min_ops_for_scale: 5000.0,
            scaling_cooldown_ms: 30000,
            ..Default::default()
        };
        let mut balancer = ShardLoadBalancer::new(2, config);

        balancer.scaling_performed(1000);

        // High load but within cooldown
        balancer.update_metrics(
            0,
            ShardMetrics {
                shard_id: 0,
                ops_per_second: 50000.0,
                ..Default::default()
            },
        );

        let decision = balancer.analyze(5000); // Only 4 seconds later
        assert_eq!(decision, ScalingDecision::NoChange);
    }

    #[test]
    fn test_load_distribution() {
        let config = LoadBalancerConfig::default();
        let mut balancer = ShardLoadBalancer::new(4, config);

        // 50% on shard 0, 50% split among others
        balancer.update_metrics(
            0,
            ShardMetrics {
                shard_id: 0,
                ops_per_second: 600.0,
                ..Default::default()
            },
        );
        for i in 1..4 {
            balancer.update_metrics(
                i,
                ShardMetrics {
                    shard_id: i,
                    ops_per_second: 200.0,
                    ..Default::default()
                },
            );
        }

        let dist = balancer.get_distribution();
        // Shard 0 should have about 50% of the load
        let shard_0_pct = dist
            .iter()
            .find(|(id, _)| *id == 0)
            .map(|(_, p)| *p)
            .unwrap_or(0.0);
        assert!(shard_0_pct > 40.0 && shard_0_pct < 60.0);
    }

    #[test]
    fn test_record_ops_smoothing() {
        let config = LoadBalancerConfig::default();
        let mut balancer = ShardLoadBalancer::new(2, config);

        // Initial update
        balancer.update_metrics(
            0,
            ShardMetrics {
                shard_id: 0,
                ops_per_second: 0.0,
                last_update_ms: 0,
                ..Default::default()
            },
        );

        // Record high ops
        balancer.record_ops(0, 1000, 1000);
        let metrics = balancer.get_shard_metrics(0).unwrap();
        // Should be smoothed (not jump directly to 1000 ops/sec)
        assert!(metrics.ops_per_second > 0.0);
        assert!(metrics.ops_per_second < 1000.0);
    }
}
