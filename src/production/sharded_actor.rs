use crate::redis::{Command, CommandExecutor, RespValue};
use crate::simulator::VirtualTime;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

use super::adaptive_replication::{AdaptiveConfig, AdaptiveReplicationManager};
use super::load_balancer::{LoadBalancerConfig, ScalingDecision, ShardLoadBalancer, ShardMetrics};
use parking_lot::RwLock;

/// Configuration for dynamic sharding behavior
#[derive(Clone, Debug)]
pub struct ShardConfig {
    /// Initial number of shards (default: num_cpus)
    pub initial_shards: usize,
    /// Minimum number of shards (default: 1)
    pub min_shards: usize,
    /// Maximum number of shards (default: 256)
    pub max_shards: usize,
    /// Enable automatic scaling based on load (default: false for now)
    pub auto_scale: bool,
    /// Enable adaptive replication for hot keys (default: false)
    pub adaptive_replication: bool,
    /// Interval for load balancing checks (milliseconds, default: 10000)
    pub load_check_interval_ms: u64,
}

impl Default for ShardConfig {
    fn default() -> Self {
        let num_cpus = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);

        ShardConfig {
            initial_shards: num_cpus,
            min_shards: 1,
            max_shards: 256,
            auto_scale: false,
            adaptive_replication: false,
            load_check_interval_ms: 10000,
        }
    }
}

impl ShardConfig {
    /// Create config with specific initial shard count
    pub fn with_shards(num_shards: usize) -> Self {
        ShardConfig {
            initial_shards: num_shards,
            ..Default::default()
        }
    }

    /// Enable adaptive features (auto-scaling and hot key detection)
    pub fn with_adaptive(mut self) -> Self {
        self.auto_scale = true;
        self.adaptive_replication = true;
        self
    }
}

#[derive(Debug)]
pub enum ShardMessage {
    Command {
        cmd: Command,
        virtual_time: VirtualTime,
        response_tx: oneshot::Sender<RespValue>,
    },
    /// Fire-and-forget batch command (no response needed)
    #[allow(dead_code)]
    BatchCommand {
        cmd: Command,
        virtual_time: VirtualTime,
    },
    EvictExpired {
        virtual_time: VirtualTime,
        response_tx: oneshot::Sender<usize>,
    },
}

pub struct ShardActor {
    executor: CommandExecutor,
    rx: mpsc::UnboundedReceiver<ShardMessage>,
    #[allow(dead_code)]
    shard_id: usize,
    #[allow(dead_code)]
    num_shards: usize,
}

impl ShardActor {
    fn new(rx: mpsc::UnboundedReceiver<ShardMessage>, simulation_start_epoch: i64, shard_id: usize, num_shards: usize) -> Self {
        debug_assert!(shard_id < num_shards, "Shard ID {} out of bounds for {} shards", shard_id, num_shards);
        let mut executor = CommandExecutor::new();
        executor.set_simulation_start_epoch(simulation_start_epoch);
        ShardActor { executor, rx, shard_id, num_shards }
    }

    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ShardMessage::Command { cmd, virtual_time, response_tx } => {
                    self.executor.set_time(virtual_time);
                    let response = self.executor.execute(&cmd);
                    let _ = response_tx.send(response);
                }
                ShardMessage::BatchCommand { cmd, virtual_time } => {
                    // Fire-and-forget: execute without sending response
                    self.executor.set_time(virtual_time);
                    let _ = self.executor.execute(&cmd);
                }
                ShardMessage::EvictExpired { virtual_time, response_tx } => {
                    let evicted = self.executor.evict_expired_direct(virtual_time);
                    let _ = response_tx.send(evicted);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ShardHandle {
    tx: mpsc::UnboundedSender<ShardMessage>,
    shard_id: usize,
}

impl ShardHandle {
    #[inline]
    async fn execute(&self, cmd: Command, virtual_time: VirtualTime) -> RespValue {
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::Command {
            cmd,
            virtual_time,
            response_tx,
        };

        if self.tx.send(msg).is_err() {
            debug_assert!(false, "Shard {} channel closed unexpectedly", self.shard_id);
            return RespValue::Error("ERR shard unavailable".to_string());
        }

        response_rx.await.unwrap_or_else(|_| {
            debug_assert!(false, "Shard {} response channel dropped", self.shard_id);
            RespValue::Error("ERR shard response failed".to_string())
        })
    }

    /// Fire-and-forget execution - no response channel allocation
    #[inline]
    #[allow(dead_code)]
    fn execute_fire_and_forget(&self, cmd: Command, virtual_time: VirtualTime) {
        let msg = ShardMessage::BatchCommand { cmd, virtual_time };
        let _ = self.tx.send(msg);
    }

    #[inline]
    async fn evict_expired(&self, virtual_time: VirtualTime) -> usize {
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::EvictExpired {
            virtual_time,
            response_tx,
        };

        if self.tx.send(msg).is_err() {
            return 0;
        }

        response_rx.await.unwrap_or(0)
    }
}

#[inline]
fn hash_key(key: &str, num_shards: usize) -> usize {
    debug_assert!(num_shards > 0, "num_shards must be positive");
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % num_shards;
    debug_assert!(idx < num_shards, "Hash produced invalid shard index");
    idx
}

/// Shared state for adaptive components (accessed from multiple tasks)
struct AdaptiveState {
    /// Hot key detection and RF management
    adaptive_manager: Option<AdaptiveReplicationManager>,
    /// Load balancing and scaling decisions
    load_balancer: Option<ShardLoadBalancer>,
}

#[derive(Clone)]
pub struct ShardedActorState {
    shards: Arc<Vec<ShardHandle>>,
    num_shards: usize,
    start_time: SystemTime,
    config: ShardConfig,
    /// Adaptive components (protected by RwLock for concurrent access)
    adaptive: Arc<RwLock<AdaptiveState>>,
}

impl ShardedActorState {
    /// Create with default configuration (num_cpus shards)
    pub fn new() -> Self {
        Self::with_config(ShardConfig::default())
    }

    /// Create with specific number of shards
    pub fn with_shards(num_shards: usize) -> Self {
        Self::with_config(ShardConfig::with_shards(num_shards))
    }

    /// Create with full configuration
    pub fn with_config(config: ShardConfig) -> Self {
        let num_shards = config.initial_shards
            .max(config.min_shards)
            .min(config.max_shards);

        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time before UNIX epoch")
            .as_secs() as i64;

        let shards: Vec<ShardHandle> = (0..num_shards)
            .map(|shard_id| {
                let (tx, rx) = mpsc::unbounded_channel();
                let actor = ShardActor::new(rx, epoch, shard_id, num_shards);
                tokio::spawn(actor.run());
                ShardHandle { tx, shard_id }
            })
            .collect();

        // Initialize adaptive components if enabled
        let adaptive_manager = if config.adaptive_replication {
            Some(AdaptiveReplicationManager::new(AdaptiveConfig::default()))
        } else {
            None
        };

        let load_balancer = if config.auto_scale {
            Some(ShardLoadBalancer::new(
                num_shards,
                LoadBalancerConfig {
                    min_shards: config.min_shards,
                    max_shards: config.max_shards,
                    ..Default::default()
                },
            ))
        } else {
            None
        };

        ShardedActorState {
            shards: Arc::new(shards),
            num_shards,
            start_time: SystemTime::now(),
            config,
            adaptive: Arc::new(RwLock::new(AdaptiveState {
                adaptive_manager,
                load_balancer,
            })),
        }
    }

    /// Get current number of shards
    pub fn num_shards(&self) -> usize {
        self.num_shards
    }

    /// Check if adaptive features are enabled
    pub fn is_adaptive_enabled(&self) -> bool {
        self.config.adaptive_replication || self.config.auto_scale
    }

    /// Observe a key access for hot key detection
    ///
    /// Call this on every key access when adaptive replication is enabled.
    #[inline]
    pub fn observe_access(&self, key: &str, is_write: bool) {
        if !self.config.adaptive_replication {
            return;
        }

        let now_ms = self.get_current_virtual_time().as_millis();
        let mut adaptive = self.adaptive.write();
        if let Some(ref mut manager) = adaptive.adaptive_manager {
            manager.observe(key, is_write, now_ms);
        }
    }

    /// Get the replication factor for a key (considers hot key status)
    pub fn get_rf_for_key(&self, key: &str) -> u8 {
        let adaptive = self.adaptive.read();
        if let Some(ref manager) = adaptive.adaptive_manager {
            manager.get_rf_for_key(key)
        } else {
            3 // Default RF
        }
    }

    /// Get current hot keys (for debugging/monitoring)
    pub fn get_hot_keys(&self) -> Vec<(String, f64)> {
        let now_ms = self.get_current_virtual_time().as_millis();
        let adaptive = self.adaptive.read();
        if let Some(ref manager) = adaptive.adaptive_manager {
            manager.get_top_hot_keys(10, now_ms)
        } else {
            Vec::new()
        }
    }

    /// Check load balancer for scaling recommendations
    pub fn check_scaling(&self) -> Option<ScalingDecision> {
        let now_ms = self.get_current_virtual_time().as_millis();
        let adaptive = self.adaptive.read();
        if let Some(ref balancer) = adaptive.load_balancer {
            let decision = balancer.analyze(now_ms);
            if decision != ScalingDecision::NoChange {
                return Some(decision);
            }
        }
        None
    }

    /// Update shard metrics for load balancing
    pub fn update_shard_metrics(&self, shard_id: usize, key_count: usize, ops_per_second: f64) {
        let now_ms = self.get_current_virtual_time().as_millis();
        let mut adaptive = self.adaptive.write();
        if let Some(ref mut balancer) = adaptive.load_balancer {
            balancer.update_metrics(shard_id, ShardMetrics {
                shard_id,
                key_count,
                ops_per_second,
                memory_bytes: 0,
                last_update_ms: now_ms,
            });
        }
    }

    /// Get adaptive statistics for INFO command
    pub fn get_adaptive_info(&self) -> String {
        let adaptive = self.adaptive.read();
        let mut info = String::new();

        info.push_str("# Adaptive\r\n");
        info.push_str(&format!("adaptive_replication:{}\r\n", self.config.adaptive_replication));
        info.push_str(&format!("auto_scale:{}\r\n", self.config.auto_scale));

        if let Some(ref manager) = adaptive.adaptive_manager {
            let stats = manager.stats();
            info.push_str(&format!("hot_keys:{}\r\n", stats.current_hot_keys));
            info.push_str(&format!("tracked_keys:{}\r\n", stats.tracked_keys));
            info.push_str(&format!("total_promotions:{}\r\n", stats.total_promotions));
            info.push_str(&format!("base_rf:{}\r\n", stats.base_rf));
            info.push_str(&format!("hot_rf:{}\r\n", stats.hot_rf));
        }

        if let Some(ref balancer) = adaptive.load_balancer {
            let stats = balancer.get_stats();
            info.push_str(&format!("total_ops_sec:{:.0}\r\n", stats.total_ops_per_sec));
            info.push_str(&format!("imbalance_ratio:{:.2}\r\n", stats.imbalance_ratio));
        }

        info
    }

    #[inline]
    fn get_current_virtual_time(&self) -> VirtualTime {
        let elapsed = self.start_time.elapsed().expect("System time went backwards");
        VirtualTime::from_millis(elapsed.as_millis() as u64)
    }

    pub async fn evict_expired_all_shards(&self) -> usize {
        let virtual_time = self.get_current_virtual_time();
        let mut total = 0usize;

        for shard in self.shards.iter() {
            total = total.saturating_add(shard.evict_expired(virtual_time).await);
        }

        total
    }

    pub async fn execute(&self, cmd: &Command) -> RespValue {
        let virtual_time = self.get_current_virtual_time();

        match cmd {
            Command::Ping => RespValue::SimpleString("PONG".to_string()),

            Command::Info => {
                let adaptive_info = self.get_adaptive_info();
                let info = format!(
                    "# Server\r\n\
                     redis_mode:tiger_style\r\n\
                     num_shards:{}\r\n\
                     architecture:actor_message_passing\r\n\
                     allocator:jemalloc\r\n\
                     \r\n\
                     # Stats\r\n\
                     current_time_ms:{}\r\n\
                     \r\n\
                     {}",
                    self.num_shards,
                    virtual_time.as_millis(),
                    adaptive_info
                );
                RespValue::BulkString(Some(info.into_bytes()))
            }

            Command::FlushDb | Command::FlushAll => {
                let mut futures = Vec::with_capacity(self.num_shards);
                for shard in self.shards.iter() {
                    futures.push(shard.execute(Command::FlushDb, virtual_time));
                }
                for future in futures {
                    let _ = future.await;
                }
                RespValue::SimpleString("OK".to_string())
            }

            Command::Keys(pattern) => {
                let mut futures = Vec::with_capacity(self.num_shards);
                for shard in self.shards.iter() {
                    futures.push(shard.execute(Command::Keys(pattern.clone()), virtual_time));
                }

                let mut all_keys: Vec<RespValue> = Vec::new();
                for future in futures {
                    if let RespValue::Array(Some(keys)) = future.await {
                        all_keys.extend(keys);
                    }
                }
                RespValue::Array(Some(all_keys))
            }

            Command::MGet(keys) => {
                let num_shards = self.num_shards;
                let futures: Vec<_> = keys.iter().map(|key| {
                    let shard_idx = hash_key(key, num_shards);
                    self.shards[shard_idx].execute(Command::Get(key.clone()), virtual_time)
                }).collect();

                // Execute all GET operations concurrently
                let results = futures::future::join_all(futures).await;
                RespValue::Array(Some(results))
            }

            Command::MSet(pairs) => {
                // Group key-value pairs by target shard (using HashMap for dynamic shard count)
                let num_shards = self.num_shards;
                let mut shard_batches: std::collections::HashMap<usize, Vec<(String, crate::redis::SDS)>> =
                    std::collections::HashMap::new();
                for (key, value) in pairs {
                    let shard_idx = hash_key(key, num_shards);
                    shard_batches.entry(shard_idx)
                        .or_default()
                        .push((key.clone(), value.clone()));
                }

                if shard_batches.is_empty() {
                    return RespValue::SimpleString("OK".to_string());
                }

                // For single-shard MSET (common case), execute directly
                if shard_batches.len() == 1 {
                    let (shard_idx, batch) = shard_batches.into_iter().next().unwrap();
                    self.shards[shard_idx].execute(Command::BatchSet(batch), virtual_time).await;
                } else {
                    // Multi-shard: send all concurrently
                    let futures: Vec<_> = shard_batches.into_iter().map(|(shard_idx, batch)| {
                        self.shards[shard_idx].execute(Command::BatchSet(batch), virtual_time)
                    }).collect();
                    futures::future::join_all(futures).await;
                }

                RespValue::SimpleString("OK".to_string())
            }

            Command::Exists(keys) => {
                let num_shards = self.num_shards;
                let futures: Vec<_> = keys.iter().map(|key| {
                    let shard_idx = hash_key(key, num_shards);
                    self.shards[shard_idx].execute(Command::Exists(vec![key.clone()]), virtual_time)
                }).collect();

                // Execute all EXISTS operations concurrently
                let results = futures::future::join_all(futures).await;
                let count: i64 = results.into_iter().filter_map(|r| {
                    if let RespValue::Integer(n) = r { Some(n) } else { None }
                }).sum();
                RespValue::Integer(count)
            }

            _ => {
                if let Some(key) = cmd.get_primary_key() {
                    let shard_idx = hash_key(key, self.num_shards);
                    debug_assert!(shard_idx < self.num_shards, "Invalid shard index for key");
                    self.shards[shard_idx].execute(cmd.clone(), virtual_time).await
                } else {
                    self.shards[0].execute(cmd.clone(), virtual_time).await
                }
            }
        }
    }
}

impl Default for ShardedActorState {
    fn default() -> Self {
        Self::new()
    }
}
