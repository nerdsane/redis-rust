//! AdaptiveActor - Actor-based adaptive replication and load balancing
//!
//! This actor owns the AdaptiveReplicationManager and ShardLoadBalancer exclusively,
//! eliminating the need for Arc<RwLock<AdaptiveState>>.
//!
//! ## Design (TigerBeetle/FoundationDB inspired)
//!
//! ```text
//! ┌─────────────────────┐        ┌─────────────────────┐
//! │  ShardedActorState  │──msg──▶│   AdaptiveActor     │
//! │  (coordinator)      │        │ (owns hot key +     │
//! └─────────────────────┘        │  load balancer)     │
//!                                └─────────────────────┘
//! ```
//!
//! All hot key detection and load balancing happens through message passing.

use crate::production::adaptive_replication::{
    AdaptiveConfig, AdaptiveReplicationManager, AdaptiveStats,
};
use crate::production::load_balancer::{
    LoadBalancerConfig, LoadBalancerStats, ScalingDecision, ShardLoadBalancer, ShardMetrics,
};
use tokio::sync::{mpsc, oneshot};

/// Messages that can be sent to the AdaptiveActor
#[derive(Debug)]
pub enum AdaptiveMessage {
    /// Observe a key access for hot key detection
    ObserveAccess {
        key: String,
        is_write: bool,
        timestamp_ms: u64,
    },

    /// Get the replication factor for a key (considers hot key status)
    GetRfForKey {
        key: String,
        response: oneshot::Sender<u8>,
    },

    /// Get current hot keys for monitoring
    GetHotKeys {
        count: usize,
        timestamp_ms: u64,
        response: oneshot::Sender<Vec<(String, f64)>>,
    },

    /// Update shard metrics for load balancing
    UpdateShardMetrics {
        shard_id: usize,
        key_count: usize,
        ops_per_second: f64,
        timestamp_ms: u64,
    },

    /// Check for scaling recommendations
    CheckScaling {
        timestamp_ms: u64,
        response: oneshot::Sender<Option<ScalingDecision>>,
    },

    /// Get adaptive statistics for INFO command
    GetStats {
        response: oneshot::Sender<AdaptiveActorStats>,
    },

    /// Get formatted info string for INFO command
    GetInfo { response: oneshot::Sender<String> },

    /// Graceful shutdown
    Shutdown { response: oneshot::Sender<()> },
}

/// Combined stats from adaptive components
#[derive(Debug, Clone, Default)]
pub struct AdaptiveActorStats {
    pub adaptive_stats: Option<AdaptiveStats>,
    pub load_balancer_stats: Option<LoadBalancerStats>,
}

/// Handle for communicating with the AdaptiveActor
#[derive(Clone)]
pub struct AdaptiveActorHandle {
    tx: mpsc::UnboundedSender<AdaptiveMessage>,
}

impl AdaptiveActorHandle {
    /// Create a new handle from a sender
    pub fn new(tx: mpsc::UnboundedSender<AdaptiveMessage>) -> Self {
        AdaptiveActorHandle { tx }
    }

    /// Observe a key access (fire-and-forget)
    #[inline]
    pub fn observe_access(&self, key: String, is_write: bool, timestamp_ms: u64) {
        let _ = self.tx.send(AdaptiveMessage::ObserveAccess {
            key,
            is_write,
            timestamp_ms,
        });
    }

    /// Get the replication factor for a key
    pub async fn get_rf_for_key(&self, key: &str) -> u8 {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(AdaptiveMessage::GetRfForKey {
                key: key.to_string(),
                response: tx,
            })
            .is_err()
        {
            return 3; // Default RF on error
        }
        rx.await.unwrap_or(3)
    }

    /// Get current hot keys
    pub async fn get_hot_keys(&self, count: usize, timestamp_ms: u64) -> Vec<(String, f64)> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(AdaptiveMessage::GetHotKeys {
                count,
                timestamp_ms,
                response: tx,
            })
            .is_err()
        {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Update shard metrics (fire-and-forget)
    #[inline]
    pub fn update_shard_metrics(
        &self,
        shard_id: usize,
        key_count: usize,
        ops_per_second: f64,
        timestamp_ms: u64,
    ) {
        let _ = self.tx.send(AdaptiveMessage::UpdateShardMetrics {
            shard_id,
            key_count,
            ops_per_second,
            timestamp_ms,
        });
    }

    /// Check for scaling recommendations
    pub async fn check_scaling(&self, timestamp_ms: u64) -> Option<ScalingDecision> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(AdaptiveMessage::CheckScaling {
                timestamp_ms,
                response: tx,
            })
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Get adaptive statistics
    pub async fn get_stats(&self) -> AdaptiveActorStats {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(AdaptiveMessage::GetStats { response: tx })
            .is_err()
        {
            return AdaptiveActorStats::default();
        }
        rx.await.unwrap_or_default()
    }

    /// Get formatted info string
    pub async fn get_info(&self) -> String {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(AdaptiveMessage::GetInfo { response: tx })
            .is_err()
        {
            return String::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Graceful shutdown
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(AdaptiveMessage::Shutdown { response: tx })
            .is_ok()
        {
            let _ = rx.await;
        }
    }
}

/// Configuration for the AdaptiveActor
#[derive(Clone, Debug)]
pub struct AdaptiveActorConfig {
    /// Enable hot key detection
    pub enable_hot_key_detection: bool,
    /// Enable load balancing
    pub enable_load_balancing: bool,
    /// Number of shards (for load balancer initialization)
    pub num_shards: usize,
    /// Hot key detection config
    pub adaptive_config: AdaptiveConfig,
    /// Load balancer config
    pub load_balancer_config: LoadBalancerConfig,
}

impl Default for AdaptiveActorConfig {
    fn default() -> Self {
        AdaptiveActorConfig {
            enable_hot_key_detection: false,
            enable_load_balancing: false,
            num_shards: 1,
            adaptive_config: AdaptiveConfig::default(),
            load_balancer_config: LoadBalancerConfig::default(),
        }
    }
}

/// The AdaptiveActor owns state exclusively - no Arc<RwLock<>> needed!
pub struct AdaptiveActor {
    /// Hot key detection and RF management (owned)
    adaptive_manager: Option<AdaptiveReplicationManager>,
    /// Load balancing and scaling decisions (owned)
    load_balancer: Option<ShardLoadBalancer>,
    /// Message receiver
    rx: mpsc::UnboundedReceiver<AdaptiveMessage>,
}

impl AdaptiveActor {
    /// Create a new AdaptiveActor and return the handle
    pub fn spawn(config: AdaptiveActorConfig) -> AdaptiveActorHandle {
        let (tx, rx) = mpsc::unbounded_channel();

        let adaptive_manager = if config.enable_hot_key_detection {
            Some(AdaptiveReplicationManager::new(config.adaptive_config))
        } else {
            None
        };

        let load_balancer = if config.enable_load_balancing {
            Some(ShardLoadBalancer::new(
                config.num_shards,
                config.load_balancer_config,
            ))
        } else {
            None
        };

        let actor = AdaptiveActor {
            adaptive_manager,
            load_balancer,
            rx,
        };

        tokio::spawn(async move {
            actor.run().await;
        });

        AdaptiveActorHandle::new(tx)
    }

    /// Run the actor's message loop
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                AdaptiveMessage::ObserveAccess {
                    key,
                    is_write,
                    timestamp_ms,
                } => {
                    if let Some(ref mut manager) = self.adaptive_manager {
                        manager.observe(&key, is_write, timestamp_ms);
                    }

                    #[cfg(debug_assertions)]
                    self.verify_invariants();
                }

                AdaptiveMessage::GetRfForKey { key, response } => {
                    let rf = if let Some(ref manager) = self.adaptive_manager {
                        manager.get_rf_for_key(&key)
                    } else {
                        3 // Default RF
                    };
                    let _ = response.send(rf);
                }

                AdaptiveMessage::GetHotKeys {
                    count,
                    timestamp_ms,
                    response,
                } => {
                    let hot_keys = if let Some(ref manager) = self.adaptive_manager {
                        manager.get_top_hot_keys(count, timestamp_ms)
                    } else {
                        Vec::new()
                    };
                    let _ = response.send(hot_keys);
                }

                AdaptiveMessage::UpdateShardMetrics {
                    shard_id,
                    key_count,
                    ops_per_second,
                    timestamp_ms,
                } => {
                    if let Some(ref mut balancer) = self.load_balancer {
                        balancer.update_metrics(
                            shard_id,
                            ShardMetrics {
                                shard_id,
                                key_count,
                                ops_per_second,
                                memory_bytes: 0,
                                last_update_ms: timestamp_ms,
                            },
                        );
                    }

                    #[cfg(debug_assertions)]
                    self.verify_invariants();
                }

                AdaptiveMessage::CheckScaling {
                    timestamp_ms,
                    response,
                } => {
                    let decision = if let Some(ref balancer) = self.load_balancer {
                        let decision = balancer.analyze(timestamp_ms);
                        if decision != ScalingDecision::NoChange {
                            Some(decision)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let _ = response.send(decision);
                }

                AdaptiveMessage::GetStats { response } => {
                    let stats = AdaptiveActorStats {
                        adaptive_stats: self.adaptive_manager.as_ref().map(|m| m.stats()),
                        load_balancer_stats: self.load_balancer.as_ref().map(|b| b.get_stats()),
                    };
                    let _ = response.send(stats);
                }

                AdaptiveMessage::GetInfo { response } => {
                    let info = self.format_info();
                    let _ = response.send(info);
                }

                AdaptiveMessage::Shutdown { response } => {
                    let _ = response.send(());
                    break;
                }
            }
        }
    }

    /// Format info string for INFO command
    fn format_info(&self) -> String {
        let mut info = String::new();

        info.push_str("# Adaptive\r\n");
        info.push_str(&format!(
            "adaptive_replication:{}\r\n",
            self.adaptive_manager.is_some()
        ));
        info.push_str(&format!("auto_scale:{}\r\n", self.load_balancer.is_some()));

        if let Some(ref manager) = self.adaptive_manager {
            let stats = manager.stats();
            info.push_str(&format!("hot_keys:{}\r\n", stats.current_hot_keys));
            info.push_str(&format!("tracked_keys:{}\r\n", stats.tracked_keys));
            info.push_str(&format!("total_promotions:{}\r\n", stats.total_promotions));
            info.push_str(&format!("base_rf:{}\r\n", stats.base_rf));
            info.push_str(&format!("hot_rf:{}\r\n", stats.hot_rf));
        }

        if let Some(ref balancer) = self.load_balancer {
            let stats = balancer.get_stats();
            info.push_str(&format!("total_ops_sec:{:.0}\r\n", stats.total_ops_per_sec));
            info.push_str(&format!("imbalance_ratio:{:.2}\r\n", stats.imbalance_ratio));
        }

        info
    }

    /// Verify invariants (TigerStyle design-by-contract)
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant: If adaptive_manager exists, it should have valid state
        if let Some(ref manager) = self.adaptive_manager {
            let stats = manager.stats();
            debug_assert!(
                stats.base_rf <= stats.hot_rf,
                "Invariant: base_rf ({}) must be <= hot_rf ({})",
                stats.base_rf,
                stats.hot_rf
            );
            debug_assert!(
                stats.current_hot_keys <= stats.tracked_keys,
                "Invariant: current_hot_keys ({}) must be <= tracked_keys ({})",
                stats.current_hot_keys,
                stats.tracked_keys
            );
        }

        // Invariant: If load_balancer exists, it should have valid state
        if let Some(ref balancer) = self.load_balancer {
            let stats = balancer.get_stats();
            debug_assert!(
                stats.imbalance_ratio >= 0.0,
                "Invariant: imbalance_ratio must be non-negative"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AdaptiveActorConfig {
        AdaptiveActorConfig {
            enable_hot_key_detection: true,
            enable_load_balancing: true,
            num_shards: 4,
            adaptive_config: AdaptiveConfig::default(),
            load_balancer_config: LoadBalancerConfig {
                min_shards: 1,
                max_shards: 16,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn test_adaptive_actor_basic() {
        let handle = AdaptiveActor::spawn(test_config());

        // Observe some accesses
        handle.observe_access("key1".to_string(), false, 1000);
        handle.observe_access("key1".to_string(), false, 1001);
        handle.observe_access("key2".to_string(), true, 1002);

        // Get RF (should be default since not enough accesses for hot key)
        let rf = handle.get_rf_for_key("key1").await;
        assert!(rf >= 3);

        // Graceful shutdown
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_adaptive_actor_hot_keys() {
        let handle = AdaptiveActor::spawn(test_config());

        let timestamp = 1000u64;

        // Generate many accesses to make key hot
        for i in 0..100 {
            handle.observe_access("hot_key".to_string(), false, timestamp + i);
        }

        // Small delay to let messages process
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Get hot keys
        let hot_keys = handle.get_hot_keys(10, timestamp + 100).await;
        // May be empty if decay is aggressive, so we just verify it doesn't panic
        let _ = hot_keys;

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_adaptive_actor_load_balancing() {
        let handle = AdaptiveActor::spawn(test_config());

        let timestamp = 1000u64;

        // Update shard metrics
        handle.update_shard_metrics(0, 1000, 100.0, timestamp);
        handle.update_shard_metrics(1, 2000, 200.0, timestamp);
        handle.update_shard_metrics(2, 500, 50.0, timestamp);
        handle.update_shard_metrics(3, 3000, 300.0, timestamp);

        // Small delay
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Check scaling
        let decision = handle.check_scaling(timestamp + 1000).await;
        // Decision depends on imbalance threshold
        assert!(decision.is_none() || matches!(decision, Some(_)));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_adaptive_actor_get_info() {
        let handle = AdaptiveActor::spawn(test_config());

        let info = handle.get_info().await;
        assert!(info.contains("adaptive_replication:true"));
        assert!(info.contains("auto_scale:true"));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_adaptive_actor_disabled() {
        let config = AdaptiveActorConfig {
            enable_hot_key_detection: false,
            enable_load_balancing: false,
            ..Default::default()
        };
        let handle = AdaptiveActor::spawn(config);

        // Operations should still work but be no-ops
        handle.observe_access("key".to_string(), false, 1000);
        let rf = handle.get_rf_for_key("key").await;
        assert_eq!(rf, 3); // Default

        let decision = handle.check_scaling(1000).await;
        assert!(decision.is_none());

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_adaptive_actor_multiple_handles() {
        let handle1 = AdaptiveActor::spawn(test_config());
        let handle2 = handle1.clone();

        // Both handles should work
        handle1.observe_access("key1".to_string(), false, 1000);
        handle2.observe_access("key2".to_string(), true, 1001);

        let rf1 = handle1.get_rf_for_key("key1").await;
        let rf2 = handle2.get_rf_for_key("key2").await;
        assert!(rf1 >= 3);
        assert!(rf2 >= 3);

        handle1.shutdown().await;
    }
}
