use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsistencyLevel {
    Eventual,
    Causal,
}

impl Default for ConsistencyLevel {
    fn default() -> Self {
        ConsistencyLevel::Eventual
    }
}

/// Replication and partitioning configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    /// Enable replication (gossip protocol)
    pub enabled: bool,
    /// Unique identifier for this replica
    pub replica_id: u64,
    /// Consistency level for reads/writes
    pub consistency_level: ConsistencyLevel,
    /// Gossip interval in milliseconds
    pub gossip_interval_ms: u64,
    /// Peer addresses for gossip (e.g., "127.0.0.1:3001")
    pub peers: Vec<String>,
    /// Number of replicas per key (Anna KVS replication factor)
    pub replication_factor: usize,

    // ========================================================================
    // Anna KVS-style partitioning settings
    // ========================================================================
    /// Enable partitioned sharding (keys distributed across nodes).
    /// When false, all nodes store all keys (full replication).
    /// When true, keys are assigned to `replication_factor` nodes via consistent hashing.
    pub partitioned_mode: bool,

    /// Enable selective gossip (only send deltas to responsible nodes).
    /// Requires `partitioned_mode: true`. Reduces network traffic from O(n) to O(RF).
    pub selective_gossip: bool,

    /// Number of virtual nodes per physical node on the hash ring.
    /// Higher values improve distribution balance but use more memory.
    /// Recommended: 100-200 for production.
    pub virtual_nodes_per_physical: u32,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        ReplicationConfig {
            enabled: false,
            replica_id: 1,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 1000,
            peers: Vec::new(),
            replication_factor: 3,
            // Partitioning disabled by default (backward compatible)
            partitioned_mode: false,
            selective_gossip: false,
            virtual_nodes_per_physical: 150,
        }
    }
}

impl ReplicationConfig {
    /// Create config for a single-node deployment (no replication)
    pub fn new_single_node() -> Self {
        Self::default()
    }

    /// Create config for a full-replication cluster (all nodes have all data)
    pub fn new_cluster(replica_id: u64, peers: Vec<String>) -> Self {
        ReplicationConfig {
            enabled: true,
            replica_id,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 1000,
            peers,
            replication_factor: 3,
            partitioned_mode: false,
            selective_gossip: false,
            virtual_nodes_per_physical: 150,
        }
    }

    /// Create config for Anna KVS-style partitioned cluster
    pub fn new_partitioned_cluster(
        replica_id: u64,
        peers: Vec<String>,
        replication_factor: usize,
    ) -> Self {
        ReplicationConfig {
            enabled: true,
            replica_id,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 1000,
            peers,
            replication_factor,
            partitioned_mode: true,
            selective_gossip: true,
            virtual_nodes_per_physical: 150,
        }
    }

    /// Enable causal consistency
    pub fn with_causal_consistency(mut self) -> Self {
        self.consistency_level = ConsistencyLevel::Causal;
        self
    }

    /// Enable partitioned mode with selective gossip
    pub fn with_partitioned_mode(mut self) -> Self {
        self.partitioned_mode = true;
        self.selective_gossip = true;
        self
    }

    /// Set the replication factor
    pub fn with_replication_factor(mut self, rf: usize) -> Self {
        self.replication_factor = rf;
        self
    }

    /// Set virtual nodes per physical node
    pub fn with_virtual_nodes(mut self, count: u32) -> Self {
        self.virtual_nodes_per_physical = count;
        self
    }

    /// Get gossip interval as Duration
    pub fn gossip_interval(&self) -> Duration {
        Duration::from_millis(self.gossip_interval_ms)
    }

    /// Check if this config uses partitioned sharding
    pub fn is_partitioned(&self) -> bool {
        self.partitioned_mode && self.enabled
    }

    /// Check if selective gossip is active
    pub fn uses_selective_gossip(&self) -> bool {
        self.selective_gossip && self.partitioned_mode && self.enabled
    }

    /// Get the total number of nodes in the cluster (self + peers)
    pub fn cluster_size(&self) -> usize {
        self.peers.len() + 1
    }
}
