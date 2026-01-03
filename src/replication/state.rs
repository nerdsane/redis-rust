use super::lattice::{LamportClock, LwwRegister, ReplicaId, VectorClock};
use super::config::ConsistencyLevel;
use crate::redis::SDS;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicatedValue {
    pub lww: LwwRegister<SDS>,
    pub vector_clock: Option<VectorClock>,
    pub expiry_ms: Option<u64>,
}

impl ReplicatedValue {
    pub fn new(replica_id: ReplicaId) -> Self {
        ReplicatedValue {
            lww: LwwRegister::new(replica_id),
            vector_clock: None,
            expiry_ms: None,
        }
    }

    pub fn with_value(value: SDS, timestamp: LamportClock) -> Self {
        ReplicatedValue {
            lww: LwwRegister::with_value(value, timestamp),
            vector_clock: None,
            expiry_ms: None,
        }
    }

    pub fn set(&mut self, value: SDS, clock: &mut LamportClock, vc: Option<&mut VectorClock>) {
        self.lww.set(value, clock);
        if let Some(vc) = vc {
            vc.increment(clock.replica_id);
            self.vector_clock = Some(vc.clone());
        }
    }

    pub fn delete(&mut self, clock: &mut LamportClock) {
        self.lww.delete(clock);
    }

    pub fn merge(&self, other: &Self) -> Self {
        let merged_lww = self.lww.merge(&other.lww);
        let merged_vc = match (&self.vector_clock, &other.vector_clock) {
            (Some(vc1), Some(vc2)) => Some(vc1.merge(vc2)),
            (Some(vc), None) | (None, Some(vc)) => Some(vc.clone()),
            (None, None) => None,
        };
        let merged_expiry = match (self.expiry_ms, other.expiry_ms) {
            (Some(e1), Some(e2)) => Some(e1.max(e2)),
            (Some(e), None) | (None, Some(e)) => Some(e),
            (None, None) => None,
        };
        ReplicatedValue {
            lww: merged_lww,
            vector_clock: merged_vc,
            expiry_ms: merged_expiry,
        }
    }

    pub fn get(&self) -> Option<&SDS> {
        self.lww.get()
    }

    pub fn is_tombstone(&self) -> bool {
        self.lww.tombstone
    }
}

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

#[derive(Debug)]
pub struct ShardReplicaState {
    pub replica_id: ReplicaId,
    pub lamport_clock: LamportClock,
    pub vector_clock: VectorClock,
    pub consistency_level: ConsistencyLevel,
    pub pending_deltas: Vec<ReplicationDelta>,
    pub replicated_keys: HashMap<String, ReplicatedValue>,
}

impl ShardReplicaState {
    pub fn new(replica_id: ReplicaId, consistency_level: ConsistencyLevel) -> Self {
        ShardReplicaState {
            replica_id,
            lamport_clock: LamportClock::new(replica_id),
            vector_clock: VectorClock::new(),
            consistency_level,
            pending_deltas: Vec::new(),
            replicated_keys: HashMap::new(),
        }
    }

    pub fn record_write(&mut self, key: String, value: SDS, expiry_ms: Option<u64>) -> ReplicationDelta {
        let mut replicated = self.replicated_keys
            .remove(&key)
            .unwrap_or_else(|| ReplicatedValue::new(self.replica_id));

        let vc = if self.consistency_level == ConsistencyLevel::Causal {
            Some(&mut self.vector_clock)
        } else {
            None
        };

        replicated.set(value, &mut self.lamport_clock, vc);
        replicated.expiry_ms = expiry_ms;

        let delta = ReplicationDelta::new(key.clone(), replicated.clone(), self.replica_id);
        self.replicated_keys.insert(key, replicated);
        self.pending_deltas.push(delta.clone());
        delta
    }

    pub fn record_delete(&mut self, key: String) -> Option<ReplicationDelta> {
        if let Some(mut replicated) = self.replicated_keys.remove(&key) {
            replicated.delete(&mut self.lamport_clock);
            let delta = ReplicationDelta::new(key.clone(), replicated.clone(), self.replica_id);
            self.replicated_keys.insert(key, replicated);
            self.pending_deltas.push(delta.clone());
            Some(delta)
        } else {
            None
        }
    }

    pub fn apply_remote_delta(&mut self, delta: ReplicationDelta) {
        self.lamport_clock.update(&delta.value.lww.timestamp);

        let existing = self.replicated_keys.remove(&delta.key);
        let merged = match existing {
            Some(local) => local.merge(&delta.value),
            None => delta.value,
        };
        self.replicated_keys.insert(delta.key, merged);
    }

    pub fn drain_pending_deltas(&mut self) -> Vec<ReplicationDelta> {
        std::mem::take(&mut self.pending_deltas)
    }

    pub fn get_replicated(&self, key: &str) -> Option<&ReplicatedValue> {
        self.replicated_keys.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_replica_write_and_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        let delta1 = state1.record_write("key1".to_string(), SDS::from_str("value1"), None);
        let delta2 = state2.record_write("key1".to_string(), SDS::from_str("value2"), None);

        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        let val1 = state1.get_replicated("key1").unwrap().get();
        let val2 = state2.get_replicated("key1").unwrap().get();
        assert_eq!(val1, val2);
    }
}
