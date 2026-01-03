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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    pub enabled: bool,
    pub replica_id: u64,
    pub consistency_level: ConsistencyLevel,
    pub gossip_interval_ms: u64,
    pub peers: Vec<String>,
    pub replication_factor: usize,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        ReplicationConfig {
            enabled: false,
            replica_id: 1,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 100,
            peers: Vec::new(),
            replication_factor: 3,
        }
    }
}

impl ReplicationConfig {
    pub fn new_single_node() -> Self {
        Self::default()
    }

    pub fn new_cluster(replica_id: u64, peers: Vec<String>) -> Self {
        ReplicationConfig {
            enabled: true,
            replica_id,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 100,
            peers,
            replication_factor: 3,
        }
    }

    pub fn with_causal_consistency(mut self) -> Self {
        self.consistency_level = ConsistencyLevel::Causal;
        self
    }

    pub fn gossip_interval(&self) -> Duration {
        Duration::from_millis(self.gossip_interval_ms)
    }
}
