use super::gossip_actor::GossipActorHandle;
use super::replicated_shard_actor::{ReplicatedShardActor, ReplicatedShardHandle};
use crate::io::{ProductionTimeSource, TimeSource};
use crate::redis::{Command, RespValue};
use crate::replication::gossip::GossipState;
use crate::replication::{ReplicaId, ReplicationConfig, ReplicationDelta};
use crate::simulator::VirtualTime;
use crate::streaming::DeltaSinkSender;
use parking_lot::RwLock;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Gossip backend for replication
///
/// Supports both legacy lock-based and new actor-based approaches.
/// The actor-based approach eliminates lock contention and follows
/// the actor model for better testability.
#[derive(Clone)]
pub enum GossipBackend {
    /// Legacy: Arc<RwLock<GossipState>> - shared mutable state with locks
    Locked(Arc<RwLock<GossipState>>),
    /// Actor-based: message passing, no locks (preferred)
    Actor(GossipActorHandle),
}

const NUM_SHARDS: usize = 16;

fn hash_key(key: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % NUM_SHARDS
}

/// Replicated sharded state with configurable time source
///
/// Generic over `T: TimeSource` for zero-cost abstraction:
/// - Production: `ProductionTimeSource` (ZST, compiles to syscall)
/// - Simulation: `SimulatedTimeSource` (virtual clock)
///
/// ## Architecture (Actor-Based)
///
/// Each shard is owned by a `ReplicatedShardActor` - no `Arc<RwLock<>>` needed!
/// All shard operations go through message passing for lock-free execution.
///
/// ## Gossip Backend
///
/// Supports both legacy lock-based and actor-based gossip:
/// - `GossipBackend::Locked`: Arc<RwLock<GossipState>> - shared mutable state
/// - `GossipBackend::Actor`: GossipActorHandle - message passing (preferred)
///
/// Use `with_gossip_actor()` or `with_gossip_actor_and_time()` for actor-based.
pub struct ReplicatedShardedState<T: TimeSource = ProductionTimeSource> {
    /// Actor handles for each shard (no locks!)
    shards: Vec<ReplicatedShardHandle>,
    config: ReplicationConfig,
    /// Gossip backend - supports both locked and actor-based modes
    gossip_backend: GossipBackend,
    /// Optional delta sink for streaming persistence
    delta_sink: Option<DeltaSinkSender>,
    /// Time source for getting current time
    time_source: T,
}

/// Production-specific constructors
impl ReplicatedShardedState<ProductionTimeSource> {
    /// Create with lock-based gossip (legacy)
    pub fn new(config: ReplicationConfig) -> Self {
        Self::with_time_source(config, ProductionTimeSource::new())
    }

    /// Create with actor-based gossip (preferred)
    ///
    /// This is the recommended constructor for production as it eliminates
    /// lock contention and follows the actor model.
    pub fn with_gossip_actor(config: ReplicationConfig, gossip_handle: GossipActorHandle) -> Self {
        Self::with_gossip_actor_and_time(config, gossip_handle, ProductionTimeSource::new())
    }
}

/// Generic implementation that works with any TimeSource
impl<T: TimeSource> ReplicatedShardedState<T> {
    /// Create with configuration and custom time source (lock-based gossip)
    pub fn with_time_source(config: ReplicationConfig, time_source: T) -> Self {
        let replica_id = ReplicaId::new(config.replica_id);
        let consistency_level = config.consistency_level;

        // Spawn actor for each shard (no locks!)
        let shards = (0..NUM_SHARDS)
            .map(|shard_id| ReplicatedShardActor::spawn(replica_id, consistency_level, shard_id))
            .collect();

        let gossip_state = Arc::new(RwLock::new(GossipState::new(config.clone())));

        ReplicatedShardedState {
            shards,
            config,
            gossip_backend: GossipBackend::Locked(gossip_state),
            delta_sink: None,
            time_source,
        }
    }

    /// Create with configuration, custom time source, and actor-based gossip (preferred)
    ///
    /// This is the recommended constructor as it eliminates lock contention
    /// and follows the actor model for better testability.
    pub fn with_gossip_actor_and_time(
        config: ReplicationConfig,
        gossip_handle: GossipActorHandle,
        time_source: T,
    ) -> Self {
        let replica_id = ReplicaId::new(config.replica_id);
        let consistency_level = config.consistency_level;

        // Spawn actor for each shard (no locks!)
        let shards = (0..NUM_SHARDS)
            .map(|shard_id| ReplicatedShardActor::spawn(replica_id, consistency_level, shard_id))
            .collect();

        ReplicatedShardedState {
            shards,
            config,
            gossip_backend: GossipBackend::Actor(gossip_handle),
            delta_sink: None,
            time_source,
        }
    }

    /// Set the delta sink for streaming persistence
    pub fn set_delta_sink(&mut self, sink: DeltaSinkSender) {
        self.delta_sink = Some(sink);
    }

    /// Clear the delta sink (for shutdown)
    pub fn clear_delta_sink(&mut self) {
        self.delta_sink = None;
    }

    /// Check if streaming persistence is enabled
    pub fn has_streaming_persistence(&self) -> bool {
        self.delta_sink.is_some()
    }

    /// Execute a command (async - uses actor message passing)
    pub async fn execute(&self, cmd: Command) -> RespValue {
        if let Some(key) = cmd.get_primary_key() {
            let shard_idx = hash_key(&key);
            let (result, delta) = self.shards[shard_idx].execute(cmd).await;

            if let Some(delta) = delta {
                // Send to gossip for replication
                if self.config.enabled {
                    match &self.gossip_backend {
                        GossipBackend::Locked(gossip_state) => {
                            let mut gossip = gossip_state.write();
                            gossip.queue_deltas(vec![delta.clone()]);
                        }
                        GossipBackend::Actor(handle) => {
                            // Actor-based: fire-and-forget, no locks!
                            handle.queue_deltas(vec![delta.clone()]);
                        }
                    }
                }

                // Send to streaming persistence if enabled
                if let Some(ref sink) = self.delta_sink {
                    // Best-effort send - don't block or error on persistence failures
                    let _ = sink.send(delta);
                }
            }

            result
        } else {
            self.execute_global(cmd).await
        }
    }

    async fn execute_global(&self, cmd: Command) -> RespValue {
        match &cmd {
            Command::Ping => RespValue::simple("PONG"),
            Command::FlushDb | Command::FlushAll => {
                // Execute on all shards concurrently
                let futures: Vec<_> = self
                    .shards
                    .iter()
                    .map(|shard| shard.execute(cmd.clone()))
                    .collect();
                futures::future::join_all(futures).await;
                RespValue::simple("OK")
            }
            Command::MSet(pairs) => {
                // Execute all SETs concurrently
                let futures: Vec<_> = pairs
                    .iter()
                    .map(|(key, value)| {
                        let shard_idx = hash_key(key);
                        let set_cmd = Command::set(key.clone(), value.clone());
                        self.shards[shard_idx].execute(set_cmd)
                    })
                    .collect();
                futures::future::join_all(futures).await;
                RespValue::simple("OK")
            }
            Command::MGet(keys) => {
                // Execute all GETs concurrently
                let futures: Vec<_> = keys
                    .iter()
                    .map(|key| {
                        let shard_idx = hash_key(key);
                        let get_cmd = Command::Get(key.clone());
                        self.shards[shard_idx].execute_readonly(get_cmd)
                    })
                    .collect();
                let results = futures::future::join_all(futures).await;
                RespValue::Array(Some(results))
            }
            Command::Exists(keys) => {
                // Execute all EXISTS concurrently
                let futures: Vec<_> = keys
                    .iter()
                    .map(|key| {
                        let shard_idx = hash_key(key);
                        let exists_cmd = Command::Exists(vec![key.clone()]);
                        self.shards[shard_idx].execute_readonly(exists_cmd)
                    })
                    .collect();
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
            Command::Keys(_pattern) => {
                // Execute on all shards concurrently
                let futures: Vec<_> = self
                    .shards
                    .iter()
                    .map(|shard| shard.execute_readonly(cmd.clone()))
                    .collect();
                let results = futures::future::join_all(futures).await;
                let mut all_keys = Vec::new();
                for result in results {
                    if let RespValue::Array(Some(keys)) = result {
                        all_keys.extend(keys);
                    }
                }
                RespValue::Array(Some(all_keys))
            }
            Command::Info => {
                let info = format!(
                    "# Replication\r\nrole:master\r\nreplica_id:{}\r\nconsistency_level:{:?}\r\nreplication_enabled:{}\r\nnum_shards:{}\r\narchitecture:actor_per_shard\r\n",
                    self.config.replica_id,
                    self.config.consistency_level,
                    self.config.enabled,
                    NUM_SHARDS
                );
                RespValue::BulkString(Some(info.into_bytes()))
            }
            _ => RespValue::err("ERR unknown command"),
        }
    }

    /// Apply remote deltas from other replicas (fire-and-forget)
    pub fn apply_remote_deltas(&self, deltas: Vec<ReplicationDelta>) {
        for delta in deltas {
            let shard_idx = hash_key(&delta.key);
            self.shards[shard_idx].apply_remote_delta(delta);
        }
    }

    /// Collect pending deltas from all shards (async)
    pub async fn collect_pending_deltas(&self) -> Vec<ReplicationDelta> {
        let futures: Vec<_> = self
            .shards
            .iter()
            .map(|shard| shard.drain_pending_deltas())
            .collect();
        let results = futures::future::join_all(futures).await;
        results.into_iter().flatten().collect()
    }

    /// Evict expired keys from all shards (async)
    ///
    /// Uses the configured TimeSource for zero-cost abstraction.
    pub async fn evict_expired_all_shards(&self) -> usize {
        let current_time_ms = self.time_source.now_millis();
        let current_time = VirtualTime::from_millis(current_time_ms);

        let futures: Vec<_> = self
            .shards
            .iter()
            .map(|shard| shard.evict_expired(current_time))
            .collect();
        let results = futures::future::join_all(futures).await;
        results.into_iter().sum()
    }

    /// Get the time source
    pub fn time_source(&self) -> &T {
        &self.time_source
    }

    /// Get the gossip backend
    ///
    /// Returns the configured gossip backend (either locked or actor-based).
    pub fn gossip_backend(&self) -> &GossipBackend {
        &self.gossip_backend
    }

    /// Get the gossip state (legacy lock-based)
    ///
    /// Returns Some if using lock-based gossip, None if using actor-based.
    /// Prefer using `gossip_backend()` or `gossip_actor_handle()` instead.
    pub fn get_gossip_state(&self) -> Option<Arc<RwLock<GossipState>>> {
        match &self.gossip_backend {
            GossipBackend::Locked(state) => Some(state.clone()),
            GossipBackend::Actor(_) => None,
        }
    }

    /// Get the gossip actor handle (actor-based)
    ///
    /// Returns Some if using actor-based gossip, None if using lock-based.
    pub fn gossip_actor_handle(&self) -> Option<GossipActorHandle> {
        match &self.gossip_backend {
            GossipBackend::Locked(_) => None,
            GossipBackend::Actor(handle) => Some(handle.clone()),
        }
    }

    /// Check if using actor-based gossip
    pub fn is_actor_based(&self) -> bool {
        matches!(&self.gossip_backend, GossipBackend::Actor(_))
    }

    pub fn config(&self) -> &ReplicationConfig {
        &self.config
    }

    pub fn num_shards(&self) -> usize {
        NUM_SHARDS
    }

    /// Create a snapshot of all replicated state for checkpointing (async)
    ///
    /// Returns a HashMap of all keys to their ReplicatedValue across all shards.
    /// This is used by the CheckpointManager to create full state snapshots.
    pub async fn snapshot_state(
        &self,
    ) -> HashMap<String, crate::replication::state::ReplicatedValue> {
        let futures: Vec<_> = self
            .shards
            .iter()
            .map(|shard| shard.get_snapshot())
            .collect();
        let results = futures::future::join_all(futures).await;

        let mut snapshot = HashMap::new();
        for shard_snapshot in results {
            snapshot.extend(shard_snapshot);
        }
        snapshot
    }

    /// Apply recovered state from persistence
    ///
    /// This is called during server startup to restore state from object store.
    /// If checkpoint_state is provided, it replaces the current state.
    /// Then all deltas are applied in order (CRDT merge is idempotent).
    pub fn apply_recovered_state(
        &self,
        checkpoint_state: Option<HashMap<String, crate::replication::state::ReplicatedValue>>,
        deltas: Vec<ReplicationDelta>,
    ) {
        // Step 1: Apply checkpoint state if present (fire-and-forget to actors)
        if let Some(state) = checkpoint_state {
            for (key, value) in state {
                let shard_idx = hash_key(&key);
                self.shards[shard_idx].apply_recovered_state(key, value);
            }
        }

        // Step 2: Apply all deltas (CRDT merge is idempotent)
        self.apply_remote_deltas(deltas);
    }

    /// Get the total number of keys across all shards (async)
    pub async fn key_count(&self) -> usize {
        let futures: Vec<_> = self
            .shards
            .iter()
            .map(|shard| shard.get_snapshot())
            .collect();
        let results = futures::future::join_all(futures).await;
        results.into_iter().map(|s| s.len()).sum()
    }
}

impl<T: TimeSource> Clone for ReplicatedShardedState<T> {
    fn clone(&self) -> Self {
        ReplicatedShardedState {
            shards: self.shards.clone(),
            config: self.config.clone(),
            gossip_backend: self.gossip_backend.clone(),
            delta_sink: self.delta_sink.clone(),
            time_source: self.time_source.clone(),
        }
    }
}
