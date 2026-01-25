//! ShardReplicaState - Per-shard replication state management

use super::crdt_value::CrdtValue;
use super::delta::ReplicationDelta;
use super::replicated_value::ReplicatedValue;
use crate::redis::SDS;
use crate::replication::config::ConsistencyLevel;
use crate::replication::lattice::{LamportClock, ReplicaId, VectorClock};
use std::collections::HashMap;

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

    pub fn record_write(
        &mut self,
        key: String,
        value: SDS,
        expiry_ms: Option<u64>,
    ) -> ReplicationDelta {
        let mut replicated = self
            .replicated_keys
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

    /// Record a hash field write (HSET)
    ///
    /// TigerStyle: Preconditions checked, postconditions verified
    pub fn record_hash_write(
        &mut self,
        key: String,
        fields: Vec<(String, SDS)>,
    ) -> ReplicationDelta {
        // TigerStyle: Preconditions
        debug_assert!(!key.is_empty(), "Precondition: key must not be empty");
        debug_assert!(!fields.is_empty(), "Precondition: fields must not be empty");

        #[cfg(debug_assertions)]
        let pre_pending_len = self.pending_deltas.len();
        #[cfg(debug_assertions)]
        let field_names: Vec<String> = fields.iter().map(|(f, _)| f.clone()).collect();

        let mut replicated = self.replicated_keys.remove(&key).unwrap_or_else(|| {
            let mut rv = ReplicatedValue::new(self.replica_id);
            rv.crdt = CrdtValue::new_hash();
            rv
        });

        // Ensure it's a hash (in case key existed as different type)
        if !replicated.is_hash() {
            replicated.crdt = CrdtValue::new_hash();
        }

        for (field, value) in fields {
            replicated.hash_set(field, value, &mut self.lamport_clock);
        }

        let delta = ReplicationDelta::new(key.clone(), replicated.clone(), self.replica_id);
        self.replicated_keys.insert(key.clone(), replicated);
        self.pending_deltas.push(delta.clone());

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert!(
                self.replicated_keys.contains_key(&key),
                "Postcondition: key '{}' must exist in replicated_keys",
                key
            );
            debug_assert!(
                self.replicated_keys
                    .get(&key)
                    .map(|v| v.is_hash())
                    .unwrap_or(false),
                "Postcondition: key '{}' must be a hash type",
                key
            );
            debug_assert_eq!(
                self.pending_deltas.len(),
                pre_pending_len + 1,
                "Postcondition: pending_deltas must increase by 1"
            );
            // Verify all fields were set
            if let Some(rv) = self.replicated_keys.get(&key) {
                for field_name in &field_names {
                    debug_assert!(
                        rv.hash_get(field_name).is_some(),
                        "Postcondition: field '{}' must exist after hash_write",
                        field_name
                    );
                }
            }
        }

        delta
    }

    /// Record a hash field delete (HDEL)
    ///
    /// TigerStyle: Preconditions checked, postconditions verified
    pub fn record_hash_delete(
        &mut self,
        key: String,
        fields: Vec<String>,
    ) -> Option<ReplicationDelta> {
        // TigerStyle: Preconditions
        debug_assert!(!key.is_empty(), "Precondition: key must not be empty");
        debug_assert!(!fields.is_empty(), "Precondition: fields must not be empty");

        #[cfg(debug_assertions)]
        let pre_pending_len = self.pending_deltas.len();

        if let Some(mut replicated) = self.replicated_keys.remove(&key) {
            if replicated.is_hash() {
                for field in &fields {
                    replicated.hash_delete(field, &mut self.lamport_clock);
                }
                let delta = ReplicationDelta::new(key.clone(), replicated.clone(), self.replica_id);
                self.replicated_keys.insert(key.clone(), replicated);
                self.pending_deltas.push(delta.clone());

                // TigerStyle: Postconditions
                #[cfg(debug_assertions)]
                {
                    debug_assert!(
                        self.replicated_keys.contains_key(&key),
                        "Postcondition: key '{}' must exist in replicated_keys",
                        key
                    );
                    debug_assert_eq!(
                        self.pending_deltas.len(),
                        pre_pending_len + 1,
                        "Postcondition: pending_deltas must increase by 1"
                    );
                    // Verify all fields were tombstoned
                    if let Some(rv) = self.replicated_keys.get(&key) {
                        for field in &fields {
                            debug_assert!(
                                rv.hash_get(field).is_none(),
                                "Postcondition: field '{}' must be tombstoned after hash_delete",
                                field
                            );
                        }
                    }
                }

                return Some(delta);
            }
            // Put back if not a hash
            self.replicated_keys.insert(key, replicated);
        }
        None
    }

    pub fn apply_remote_delta(&mut self, delta: ReplicationDelta) {
        // Update our clock from the delta's timestamp
        self.lamport_clock.update(&delta.value.timestamp);

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
