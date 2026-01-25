//! ReplicationDelta - Delta for CRDT replication

use super::replicated_value::ReplicatedValue;
use crate::replication::lattice::ReplicaId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationDelta {
    pub key: String,
    pub value: ReplicatedValue,
    pub source_replica: ReplicaId,
}

impl ReplicationDelta {
    pub fn new(key: String, value: ReplicatedValue, source_replica: ReplicaId) -> Self {
        ReplicationDelta {
            key,
            value,
            source_replica,
        }
    }
}
