use crate::io::{ProductionTimeSource, TimeSource};
use crate::redis::{Command, CommandExecutor, RespValue};
use crate::simulator::VirtualTime;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

// P4 optimization: Use AHash for faster shard routing
#[cfg(feature = "opt-fxhash-routing")]
use ahash::AHasher;

#[cfg(not(feature = "opt-fxhash-routing"))]
use std::collections::hash_map::DefaultHasher;

use super::adaptive_actor::{AdaptiveActor, AdaptiveActorConfig, AdaptiveActorHandle};
use super::load_balancer::ScalingDecision;
use super::perf_config::PerformanceConfig;
use super::response_pool::{response_future, ResponsePool, ResponseSlot};

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
    /// Fast path for GET - avoids Command enum overhead
    FastGet {
        key: bytes::Bytes,
        response_tx: oneshot::Sender<RespValue>,
    },
    /// Fast path for SET - avoids Command enum overhead
    FastSet {
        key: bytes::Bytes,
        value: bytes::Bytes,
        response_tx: oneshot::Sender<RespValue>,
    },
    /// Fast batch GET - multiple keys in single message for pipelining
    FastBatchGet {
        keys: Vec<bytes::Bytes>,
        response_tx: oneshot::Sender<Vec<RespValue>>,
    },
    /// Fast batch SET - multiple key-value pairs in single message for pipelining
    FastBatchSet {
        pairs: Vec<(bytes::Bytes, bytes::Bytes)>,
        response_tx: oneshot::Sender<Vec<RespValue>>,
    },
    /// Pooled fast GET - uses response slot instead of oneshot channel
    PooledFastGet {
        key: bytes::Bytes,
        response_slot: Arc<ResponseSlot<RespValue>>,
    },
    /// Pooled fast SET - uses response slot instead of oneshot channel
    PooledFastSet {
        key: bytes::Bytes,
        value: bytes::Bytes,
        response_slot: Arc<ResponseSlot<RespValue>>,
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
    fn new(
        rx: mpsc::UnboundedReceiver<ShardMessage>,
        simulation_start_epoch: i64,
        shard_id: usize,
        num_shards: usize,
    ) -> Self {
        debug_assert!(
            shard_id < num_shards,
            "Shard ID {} out of bounds for {} shards",
            shard_id,
            num_shards
        );
        let mut executor = CommandExecutor::new();
        executor.set_simulation_start_epoch(simulation_start_epoch);
        ShardActor {
            executor,
            rx,
            shard_id,
            num_shards,
        }
    }

    /// Create a new ShardActor with a shared script cache
    ///
    /// This allows all shards to share a single script cache for multi-shard Lua support.
    fn new_with_shared_scripts(
        rx: mpsc::UnboundedReceiver<ShardMessage>,
        simulation_start_epoch: i64,
        shard_id: usize,
        num_shards: usize,
        shared_script_cache: crate::redis::lua::SharedScriptCache,
    ) -> Self {
        debug_assert!(
            shard_id < num_shards,
            "Shard ID {} out of bounds for {} shards",
            shard_id,
            num_shards
        );
        let mut executor = CommandExecutor::with_shared_script_cache(shared_script_cache);
        executor.set_simulation_start_epoch(simulation_start_epoch);
        ShardActor {
            executor,
            rx,
            shard_id,
            num_shards,
        }
    }

    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ShardMessage::Command {
                    cmd,
                    virtual_time,
                    response_tx,
                } => {
                    self.executor.set_time(virtual_time);
                    let response = self.executor.execute(&cmd);
                    let _ = response_tx.send(response);
                }
                ShardMessage::BatchCommand { cmd, virtual_time } => {
                    // Fire-and-forget: execute without sending response
                    self.executor.set_time(virtual_time);
                    let _ = self.executor.execute(&cmd);
                }
                ShardMessage::EvictExpired {
                    virtual_time,
                    response_tx,
                } => {
                    let evicted = self.executor.evict_expired_direct(virtual_time);
                    let _ = response_tx.send(evicted);
                }
                ShardMessage::FastGet { key, response_tx } => {
                    // Fast path: direct GET without Command enum overhead
                    let key_str = unsafe { std::str::from_utf8_unchecked(&key) };
                    let response = self.executor.get_direct(key_str);
                    let _ = response_tx.send(response);
                }
                ShardMessage::FastSet {
                    key,
                    value,
                    response_tx,
                } => {
                    // Fast path: direct SET without Command enum overhead
                    let key_str = unsafe { std::str::from_utf8_unchecked(&key) };
                    let response = self.executor.set_direct(key_str, &value);
                    let _ = response_tx.send(response);
                }
                ShardMessage::FastBatchGet { keys, response_tx } => {
                    // Batch GET: process multiple keys in single message
                    let mut results = Vec::with_capacity(keys.len());
                    for key in keys {
                        let key_str = unsafe { std::str::from_utf8_unchecked(&key) };
                        results.push(self.executor.get_direct(key_str));
                    }
                    let _ = response_tx.send(results);
                }
                ShardMessage::FastBatchSet { pairs, response_tx } => {
                    // Batch SET: process multiple key-value pairs in single message
                    let mut results = Vec::with_capacity(pairs.len());
                    for (key, value) in pairs {
                        let key_str = unsafe { std::str::from_utf8_unchecked(&key) };
                        results.push(self.executor.set_direct(key_str, &value));
                    }
                    let _ = response_tx.send(results);
                }
                ShardMessage::PooledFastGet { key, response_slot } => {
                    // Pooled fast GET: uses response slot instead of oneshot
                    let key_str = unsafe { std::str::from_utf8_unchecked(&key) };
                    let response = self.executor.get_direct(key_str);
                    response_slot.send(response);
                }
                ShardMessage::PooledFastSet {
                    key,
                    value,
                    response_slot,
                } => {
                    // Pooled fast SET: uses response slot instead of oneshot
                    let key_str = unsafe { std::str::from_utf8_unchecked(&key) };
                    let response = self.executor.set_direct(key_str, &value);
                    response_slot.send(response);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ShardHandle {
    tx: mpsc::UnboundedSender<ShardMessage>,
    shard_id: usize,
    /// Response pool for reducing channel allocation overhead
    response_pool: Arc<ResponsePool<RespValue>>,
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
            return RespValue::err("ERR shard unavailable");
        }

        response_rx.await.unwrap_or_else(|_| {
            debug_assert!(false, "Shard {} response channel dropped", self.shard_id);
            RespValue::err("ERR shard response failed")
        })
    }

    /// Fire-and-forget execution - no response channel allocation
    #[inline]
    #[allow(dead_code)]
    fn execute_fire_and_forget(&self, cmd: Command, virtual_time: VirtualTime) {
        let msg = ShardMessage::BatchCommand { cmd, virtual_time };
        let _ = self.tx.send(msg);
    }

    /// Fast path GET - avoids Command enum allocation
    #[inline]
    pub async fn fast_get(&self, key: bytes::Bytes) -> RespValue {
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::FastGet { key, response_tx };

        if self.tx.send(msg).is_err() {
            return RespValue::err("ERR shard unavailable");
        }

        response_rx
            .await
            .unwrap_or_else(|_| RespValue::err("ERR shard response failed"))
    }

    /// Fast path SET - avoids Command enum allocation
    #[inline]
    pub async fn fast_set(&self, key: bytes::Bytes, value: bytes::Bytes) -> RespValue {
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::FastSet {
            key,
            value,
            response_tx,
        };

        if self.tx.send(msg).is_err() {
            return RespValue::err("ERR shard unavailable");
        }

        response_rx
            .await
            .unwrap_or_else(|_| RespValue::err("ERR shard response failed"))
    }

    /// Pooled fast GET - uses response pool to avoid channel allocation
    #[inline]
    pub async fn pooled_fast_get(&self, key: bytes::Bytes) -> RespValue {
        let response_slot = self.response_pool.acquire();
        let msg = ShardMessage::PooledFastGet {
            key,
            response_slot: response_slot.clone(),
        };

        if self.tx.send(msg).is_err() {
            self.response_pool.release(response_slot);
            return RespValue::err("ERR shard unavailable");
        }

        let result = response_future(response_slot.clone()).await;
        self.response_pool.release(response_slot);
        result
    }

    /// Pooled fast SET - uses response pool to avoid channel allocation
    #[inline]
    pub async fn pooled_fast_set(&self, key: bytes::Bytes, value: bytes::Bytes) -> RespValue {
        let response_slot = self.response_pool.acquire();
        let msg = ShardMessage::PooledFastSet {
            key,
            value,
            response_slot: response_slot.clone(),
        };

        if self.tx.send(msg).is_err() {
            self.response_pool.release(response_slot);
            return RespValue::err("ERR shard unavailable");
        }

        let result = response_future(response_slot.clone()).await;
        self.response_pool.release(response_slot);
        result
    }

    /// Fast batch GET - multiple keys in single actor message
    #[inline]
    pub async fn fast_batch_get(&self, keys: Vec<bytes::Bytes>) -> Vec<RespValue> {
        if keys.is_empty() {
            return Vec::new();
        }
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::FastBatchGet { keys, response_tx };

        if self.tx.send(msg).is_err() {
            return vec![RespValue::err("ERR shard unavailable")];
        }

        response_rx
            .await
            .unwrap_or_else(|_| vec![RespValue::err("ERR shard response failed")])
    }

    /// Fast batch SET - multiple key-value pairs in single actor message
    #[inline]
    pub async fn fast_batch_set(&self, pairs: Vec<(bytes::Bytes, bytes::Bytes)>) -> Vec<RespValue> {
        if pairs.is_empty() {
            return Vec::new();
        }
        let (response_tx, response_rx) = oneshot::channel();
        let msg = ShardMessage::FastBatchSet { pairs, response_tx };

        if self.tx.send(msg).is_err() {
            return vec![RespValue::err("ERR shard unavailable")];
        }

        response_rx
            .await
            .unwrap_or_else(|_| vec![RespValue::err("ERR shard response failed")])
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

/// Hash key string to shard index
/// P4 optimization: Use AHash when opt-fxhash-routing is enabled
#[inline]
fn hash_key(key: &str, num_shards: usize) -> usize {
    debug_assert!(num_shards > 0, "num_shards must be positive");

    #[cfg(feature = "opt-fxhash-routing")]
    let mut hasher = AHasher::default();

    #[cfg(not(feature = "opt-fxhash-routing"))]
    let mut hasher = DefaultHasher::new();

    key.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % num_shards;
    debug_assert!(idx < num_shards, "Hash produced invalid shard index");
    idx
}

/// Fast path: hash key bytes directly without UTF-8 validation overhead
/// P4 optimization: Use AHash when opt-fxhash-routing is enabled
#[inline]
fn hash_key_bytes(key: &[u8], num_shards: usize) -> usize {
    debug_assert!(num_shards > 0, "num_shards must be positive");

    #[cfg(feature = "opt-fxhash-routing")]
    let mut hasher = AHasher::default();

    #[cfg(not(feature = "opt-fxhash-routing"))]
    let mut hasher = DefaultHasher::new();

    key.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % num_shards;
    debug_assert!(idx < num_shards, "Hash produced invalid shard index");
    idx
}

/// Pool configuration defaults (used when no PerformanceConfig provided)
const DEFAULT_RESPONSE_POOL_CAPACITY: usize = 256;
const DEFAULT_RESPONSE_POOL_PREWARM: usize = 64;

/// Sharded actor state with configurable time source
///
/// Generic over `T: TimeSource` for zero-cost abstraction:
/// - Production: `ProductionTimeSource` (ZST, compiles to syscall)
/// - Simulation: `SimulatedTimeSource` (virtual clock)
#[derive(Clone)]
pub struct ShardedActorState<T: TimeSource = ProductionTimeSource> {
    shards: Arc<Vec<ShardHandle>>,
    num_shards: usize,
    /// Timestamp (in millis) when this state was created
    start_millis: u64,
    /// Time source for getting current time
    time_source: T,
    config: ShardConfig,
    /// Adaptive components via actor handle (no locks!)
    adaptive_handle: Option<AdaptiveActorHandle>,
    /// Shared response pool for all shards
    response_pool: Arc<ResponsePool<RespValue>>,
    /// Shared script cache for Lua scripts (allows SCRIPT LOAD to work across all shards)
    #[allow(dead_code)]
    shared_script_cache: crate::redis::lua::SharedScriptCache,
}

/// Production-specific constructors (use ProductionTimeSource)
impl ShardedActorState<ProductionTimeSource> {
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
        Self::with_config_and_time_source(config, ProductionTimeSource::new())
    }

    /// Create with PerformanceConfig (for evolutionary optimization)
    ///
    /// This constructor allows tuning all performance-critical parameters
    /// via the external configuration file.
    pub fn with_perf_config(perf_config: &PerformanceConfig) -> Self {
        let shard_config = ShardConfig::with_shards(perf_config.num_shards);
        Self::with_perf_config_and_time_source(
            perf_config,
            shard_config,
            ProductionTimeSource::new(),
        )
    }
}

/// Generic implementation that works with any TimeSource
impl<T: TimeSource> ShardedActorState<T> {
    /// Create with configuration and custom time source
    ///
    /// This is the main constructor - all other constructors delegate to this.
    pub fn with_config_and_time_source(config: ShardConfig, time_source: T) -> Self {
        let num_shards = config
            .initial_shards
            .max(config.min_shards)
            .min(config.max_shards);

        // Get epoch from time source (zero-cost for ProductionTimeSource)
        let start_millis = time_source.now_millis();
        let epoch = (start_millis / 1000) as i64;

        // Create shared response pool for all connections (using defaults)
        let response_pool = Arc::new(ResponsePool::new(
            DEFAULT_RESPONSE_POOL_CAPACITY,
            DEFAULT_RESPONSE_POOL_PREWARM,
        ));

        // Create shared script cache for all shards (enables multi-shard Lua support)
        let shared_script_cache = crate::redis::lua::SharedScriptCache::new();

        let shards: Vec<ShardHandle> = (0..num_shards)
            .map(|shard_id| {
                let (tx, rx) = mpsc::unbounded_channel();
                // Use shared script cache for all shards
                let actor = ShardActor::new_with_shared_scripts(
                    rx,
                    epoch,
                    shard_id,
                    num_shards,
                    shared_script_cache.clone(),
                );
                tokio::spawn(actor.run());
                ShardHandle {
                    tx,
                    shard_id,
                    response_pool: response_pool.clone(),
                }
            })
            .collect();

        // Spawn adaptive actor if any adaptive features are enabled
        let adaptive_handle = if config.adaptive_replication || config.auto_scale {
            Some(AdaptiveActor::spawn(AdaptiveActorConfig {
                enable_hot_key_detection: config.adaptive_replication,
                enable_load_balancing: config.auto_scale,
                num_shards,
                adaptive_config: Default::default(),
                load_balancer_config: super::load_balancer::LoadBalancerConfig {
                    min_shards: config.min_shards,
                    max_shards: config.max_shards,
                    ..Default::default()
                },
            }))
        } else {
            None
        };

        ShardedActorState {
            shards: Arc::new(shards),
            num_shards,
            start_millis,
            time_source,
            config,
            adaptive_handle,
            response_pool,
            shared_script_cache,
        }
    }

    /// Create with PerformanceConfig and custom time source
    ///
    /// This constructor allows full control over all tunable parameters
    /// for evolutionary optimization.
    pub fn with_perf_config_and_time_source(
        perf_config: &PerformanceConfig,
        shard_config: ShardConfig,
        time_source: T,
    ) -> Self {
        let num_shards = shard_config
            .initial_shards
            .max(shard_config.min_shards)
            .min(shard_config.max_shards);

        // Get epoch from time source
        let start_millis = time_source.now_millis();
        let epoch = (start_millis / 1000) as i64;

        // Create shared response pool with PerformanceConfig values
        let response_pool = Arc::new(ResponsePool::new(
            perf_config.response_pool.capacity,
            perf_config.response_pool.prewarm,
        ));

        // Create shared script cache for all shards (enables multi-shard Lua support)
        let shared_script_cache = crate::redis::lua::SharedScriptCache::new();

        let shards: Vec<ShardHandle> = (0..num_shards)
            .map(|shard_id| {
                let (tx, rx) = mpsc::unbounded_channel();
                // Use shared script cache for all shards
                let actor = ShardActor::new_with_shared_scripts(
                    rx,
                    epoch,
                    shard_id,
                    num_shards,
                    shared_script_cache.clone(),
                );
                tokio::spawn(actor.run());
                ShardHandle {
                    tx,
                    shard_id,
                    response_pool: response_pool.clone(),
                }
            })
            .collect();

        // Spawn adaptive actor if any adaptive features are enabled
        let adaptive_handle = if shard_config.adaptive_replication || shard_config.auto_scale {
            Some(AdaptiveActor::spawn(AdaptiveActorConfig {
                enable_hot_key_detection: shard_config.adaptive_replication,
                enable_load_balancing: shard_config.auto_scale,
                num_shards,
                adaptive_config: Default::default(),
                load_balancer_config: super::load_balancer::LoadBalancerConfig {
                    min_shards: shard_config.min_shards,
                    max_shards: shard_config.max_shards,
                    ..Default::default()
                },
            }))
        } else {
            None
        };

        ShardedActorState {
            shards: Arc::new(shards),
            num_shards,
            start_millis,
            time_source,
            config: shard_config,
            adaptive_handle,
            response_pool,
            shared_script_cache,
        }
    }

    /// Get current number of shards
    pub fn num_shards(&self) -> usize {
        self.num_shards
    }

    /// Get the time source
    pub fn time_source(&self) -> &T {
        &self.time_source
    }

    /// Check if adaptive features are enabled
    pub fn is_adaptive_enabled(&self) -> bool {
        self.config.adaptive_replication || self.config.auto_scale
    }

    /// Observe a key access for hot key detection (fire-and-forget)
    ///
    /// Call this on every key access when adaptive replication is enabled.
    /// This is non-blocking - sends to actor without waiting for response.
    #[inline]
    pub fn observe_access(&self, key: &str, is_write: bool) {
        if let Some(ref handle) = self.adaptive_handle {
            let now_ms = self.get_current_virtual_time().as_millis();
            handle.observe_access(key.to_string(), is_write, now_ms);
        }
    }

    /// Get the replication factor for a key (considers hot key status)
    ///
    /// Async because it queries the adaptive actor via message passing.
    pub async fn get_rf_for_key(&self, key: &str) -> u8 {
        if let Some(ref handle) = self.adaptive_handle {
            handle.get_rf_for_key(key).await
        } else {
            3 // Default RF
        }
    }

    /// Get current hot keys (for debugging/monitoring)
    ///
    /// Async because it queries the adaptive actor via message passing.
    pub async fn get_hot_keys(&self) -> Vec<(String, f64)> {
        if let Some(ref handle) = self.adaptive_handle {
            let now_ms = self.get_current_virtual_time().as_millis();
            handle.get_hot_keys(10, now_ms).await
        } else {
            Vec::new()
        }
    }

    /// Check load balancer for scaling recommendations
    ///
    /// Async because it queries the adaptive actor via message passing.
    pub async fn check_scaling(&self) -> Option<ScalingDecision> {
        if let Some(ref handle) = self.adaptive_handle {
            let now_ms = self.get_current_virtual_time().as_millis();
            handle.check_scaling(now_ms).await
        } else {
            None
        }
    }

    /// Update shard metrics for load balancing (fire-and-forget)
    ///
    /// This is non-blocking - sends to actor without waiting for response.
    pub fn update_shard_metrics(&self, shard_id: usize, key_count: usize, ops_per_second: f64) {
        if let Some(ref handle) = self.adaptive_handle {
            let now_ms = self.get_current_virtual_time().as_millis();
            handle.update_shard_metrics(shard_id, key_count, ops_per_second, now_ms);
        }
    }

    /// Get adaptive statistics for INFO command
    ///
    /// Async because it queries the adaptive actor via message passing.
    pub async fn get_adaptive_info(&self) -> String {
        let mut info = String::new();

        info.push_str("# Adaptive\r\n");
        info.push_str(&format!(
            "adaptive_replication:{}\r\n",
            self.config.adaptive_replication
        ));
        info.push_str(&format!("auto_scale:{}\r\n", self.config.auto_scale));

        if let Some(ref handle) = self.adaptive_handle {
            // Query actor for stats - this is message-based, no locks!
            let actor_info = handle.get_info().await;
            // The actor formats its own info, append it (skip header since we already added it)
            for line in actor_info.lines() {
                if !line.starts_with("# Adaptive")
                    && !line.starts_with("adaptive_replication:")
                    && !line.starts_with("auto_scale:")
                {
                    info.push_str(line);
                    info.push_str("\r\n");
                }
            }
        }

        info
    }

    /// Get the adaptive actor handle (for direct access if needed)
    pub fn adaptive_handle(&self) -> Option<&AdaptiveActorHandle> {
        self.adaptive_handle.as_ref()
    }

    /// Get current virtual time (elapsed since creation)
    ///
    /// Uses the configured TimeSource for zero-cost abstraction:
    /// - Production: direct syscall
    /// - Simulation: reads virtual clock
    #[inline]
    fn get_current_virtual_time(&self) -> VirtualTime {
        let now = self.time_source.now_millis();
        // Saturating sub handles clock going backwards gracefully
        let elapsed_ms = now.saturating_sub(self.start_millis);
        VirtualTime::from_millis(elapsed_ms)
    }

    pub async fn evict_expired_all_shards(&self) -> usize {
        let virtual_time = self.get_current_virtual_time();
        let mut total = 0usize;

        for shard in self.shards.iter() {
            total = total.saturating_add(shard.evict_expired(virtual_time).await);
        }

        total
    }

    /// Fast path GET - bypasses Command enum for lower overhead
    ///
    /// Uses bytes::Bytes to avoid String allocation. The key is hashed
    /// directly from bytes and routed to the appropriate shard.
    #[inline]
    pub async fn fast_get(&self, key: bytes::Bytes) -> RespValue {
        let shard_idx = hash_key_bytes(&key, self.num_shards);
        debug_assert!(shard_idx < self.shards.len(), "Shard index out of bounds");
        self.shards[shard_idx].fast_get(key).await
    }

    /// Fast path SET - bypasses Command enum for lower overhead
    ///
    /// Uses bytes::Bytes for both key and value to minimize allocations.
    /// The key is hashed directly from bytes and routed to the appropriate shard.
    #[inline]
    pub async fn fast_set(&self, key: bytes::Bytes, value: bytes::Bytes) -> RespValue {
        let shard_idx = hash_key_bytes(&key, self.num_shards);
        debug_assert!(shard_idx < self.shards.len(), "Shard index out of bounds");
        self.shards[shard_idx].fast_set(key, value).await
    }

    /// Pooled fast GET - uses response pool to avoid channel allocation
    ///
    /// This is the recommended method for single GET commands as it
    /// eliminates oneshot channel allocation overhead.
    #[inline]
    pub async fn pooled_fast_get(&self, key: bytes::Bytes) -> RespValue {
        let shard_idx = hash_key_bytes(&key, self.num_shards);
        debug_assert!(shard_idx < self.shards.len(), "Shard index out of bounds");
        self.shards[shard_idx].pooled_fast_get(key).await
    }

    /// Pooled fast SET - uses response pool to avoid channel allocation
    ///
    /// This is the recommended method for single SET commands as it
    /// eliminates oneshot channel allocation overhead.
    #[inline]
    pub async fn pooled_fast_set(&self, key: bytes::Bytes, value: bytes::Bytes) -> RespValue {
        let shard_idx = hash_key_bytes(&key, self.num_shards);
        debug_assert!(shard_idx < self.shards.len(), "Shard index out of bounds");
        self.shards[shard_idx].pooled_fast_set(key, value).await
    }

    /// Execute batched GETs concurrently across shards
    ///
    /// Groups keys by shard, sends batch requests concurrently, and reconstructs
    /// results in original order. This reduces channel round-trips from N to num_shards.
    ///
    /// Returns Vec<RespValue> in the same order as input keys.
    pub async fn fast_batch_get_pipeline(&self, keys: Vec<bytes::Bytes>) -> Vec<RespValue> {
        if keys.is_empty() {
            return Vec::new();
        }

        // Group keys by shard, tracking original indices for result reconstruction
        let mut shard_batches: Vec<Vec<(usize, bytes::Bytes)>> = vec![Vec::new(); self.num_shards];
        for (idx, key) in keys.iter().enumerate() {
            let shard_idx = hash_key_bytes(key, self.num_shards);
            shard_batches[shard_idx].push((idx, key.clone()));
        }

        // Prepare results vector
        let mut results = vec![RespValue::BulkString(None); keys.len()];

        // Send batch requests to all non-empty shards concurrently
        let mut futures = Vec::new();
        for (shard_idx, batch) in shard_batches.into_iter().enumerate() {
            if !batch.is_empty() {
                let indices: Vec<usize> = batch.iter().map(|(idx, _)| *idx).collect();
                let shard_keys: Vec<bytes::Bytes> = batch.into_iter().map(|(_, key)| key).collect();
                let shard = &self.shards[shard_idx];
                futures.push(async move {
                    let shard_results = shard.fast_batch_get(shard_keys).await;
                    (indices, shard_results)
                });
            }
        }

        // Await all shard responses concurrently
        let all_results = futures::future::join_all(futures).await;

        // Reconstruct results in original order
        for (indices, shard_results) in all_results {
            for (i, resp) in indices.into_iter().zip(shard_results.into_iter()) {
                results[i] = resp;
            }
        }

        results
    }

    /// Execute batched SETs concurrently across shards
    ///
    /// Groups key-value pairs by shard, sends batch requests concurrently, and
    /// reconstructs results in original order. This reduces channel round-trips
    /// from N to num_shards.
    ///
    /// Returns Vec<RespValue> in the same order as input pairs.
    pub async fn fast_batch_set_pipeline(
        &self,
        pairs: Vec<(bytes::Bytes, bytes::Bytes)>,
    ) -> Vec<RespValue> {
        if pairs.is_empty() {
            return Vec::new();
        }

        // Group pairs by shard, tracking original indices for result reconstruction
        let mut shard_batches: Vec<Vec<(usize, bytes::Bytes, bytes::Bytes)>> =
            vec![Vec::new(); self.num_shards];
        for (idx, (key, value)) in pairs.iter().enumerate() {
            let shard_idx = hash_key_bytes(key, self.num_shards);
            shard_batches[shard_idx].push((idx, key.clone(), value.clone()));
        }

        // Prepare results vector (default to OK for SETs)
        let mut results = vec![RespValue::simple("OK"); pairs.len()];

        // Send batch requests to all non-empty shards concurrently
        let mut futures = Vec::new();
        for (shard_idx, batch) in shard_batches.into_iter().enumerate() {
            if !batch.is_empty() {
                let indices: Vec<usize> = batch.iter().map(|(idx, _, _)| *idx).collect();
                let shard_pairs: Vec<(bytes::Bytes, bytes::Bytes)> = batch
                    .into_iter()
                    .map(|(_, key, value)| (key, value))
                    .collect();
                let shard = &self.shards[shard_idx];
                futures.push(async move {
                    let shard_results = shard.fast_batch_set(shard_pairs).await;
                    (indices, shard_results)
                });
            }
        }

        // Await all shard responses concurrently
        let all_results = futures::future::join_all(futures).await;

        // Reconstruct results in original order
        for (indices, shard_results) in all_results {
            for (i, resp) in indices.into_iter().zip(shard_results.into_iter()) {
                results[i] = resp;
            }
        }

        results
    }

    pub async fn execute(&self, cmd: &Command) -> RespValue {
        let virtual_time = self.get_current_virtual_time();

        match cmd {
            Command::Ping => RespValue::simple("PONG"),

            Command::Info => {
                let adaptive_info = self.get_adaptive_info().await;
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
                RespValue::simple("OK")
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
                // Optimization: Group keys by shard to reduce actor messages (Abseil Tip #5)
                let num_shards = self.num_shards;

                // Build shard batches: (shard_idx, original_indices, keys)
                let mut shard_batches: std::collections::HashMap<usize, (Vec<usize>, Vec<String>)> =
                    std::collections::HashMap::new();
                for (original_idx, key) in keys.iter().enumerate() {
                    let shard_idx = hash_key(key, num_shards);
                    let entry = shard_batches
                        .entry(shard_idx)
                        .or_insert_with(|| (Vec::new(), Vec::new()));
                    entry.0.push(original_idx);
                    entry.1.push(key.clone());
                }

                // Execute batched gets concurrently
                let futures: Vec<_> = shard_batches
                    .iter()
                    .map(|(&shard_idx, (_, batch_keys))| {
                        self.shards[shard_idx]
                            .execute(Command::BatchGet(batch_keys.clone()), virtual_time)
                    })
                    .collect();

                let batch_results = futures::future::join_all(futures).await;

                // Reconstruct results in original key order
                let mut results = vec![RespValue::BulkString(None); keys.len()];
                for ((_, (original_indices, _)), batch_result) in
                    shard_batches.into_iter().zip(batch_results)
                {
                    if let RespValue::Array(Some(values)) = batch_result {
                        for (idx, value) in original_indices.into_iter().zip(values) {
                            results[idx] = value;
                        }
                    }
                }

                RespValue::Array(Some(results))
            }

            Command::MSet(pairs) => {
                // Group key-value pairs by target shard (using HashMap for dynamic shard count)
                let num_shards = self.num_shards;
                let mut shard_batches: std::collections::HashMap<
                    usize,
                    Vec<(String, crate::redis::SDS)>,
                > = std::collections::HashMap::new();
                for (key, value) in pairs {
                    let shard_idx = hash_key(key, num_shards);
                    shard_batches
                        .entry(shard_idx)
                        .or_default()
                        .push((key.clone(), value.clone()));
                }

                if shard_batches.is_empty() {
                    return RespValue::simple("OK");
                }

                // For single-shard MSET (common case), execute directly
                if shard_batches.len() == 1 {
                    // TigerStyle: Use explicit pattern match instead of unwrap
                    if let Some((shard_idx, batch)) = shard_batches.into_iter().next() {
                        self.shards[shard_idx]
                            .execute(Command::BatchSet(batch), virtual_time)
                            .await;
                    } else {
                        debug_assert!(
                            false,
                            "Invariant violated: single-element iterator must yield one item"
                        );
                    }
                } else {
                    // Multi-shard: send all concurrently
                    let futures: Vec<_> = shard_batches
                        .into_iter()
                        .map(|(shard_idx, batch)| {
                            self.shards[shard_idx].execute(Command::BatchSet(batch), virtual_time)
                        })
                        .collect();
                    futures::future::join_all(futures).await;
                }

                RespValue::simple("OK")
            }

            Command::Exists(keys) => {
                let num_shards = self.num_shards;
                let futures: Vec<_> = keys
                    .iter()
                    .map(|key| {
                        let shard_idx = hash_key(key, num_shards);
                        self.shards[shard_idx]
                            .execute(Command::Exists(vec![key.clone()]), virtual_time)
                    })
                    .collect();

                // Execute all EXISTS operations concurrently
                let results = futures::future::join_all(futures).await;
                let count: i64 = results
                    .into_iter()
                    .filter_map(|r| {
                        if let RespValue::Integer(n) = r {
                            Some(n)
                        } else {
                            None
                        }
                    })
                    .sum();
                RespValue::Integer(count)
            }

            _ => {
                if let Some(key) = cmd.get_primary_key() {
                    let shard_idx = hash_key(key, self.num_shards);
                    debug_assert!(shard_idx < self.num_shards, "Invalid shard index for key");
                    self.shards[shard_idx]
                        .execute(cmd.clone(), virtual_time)
                        .await
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
