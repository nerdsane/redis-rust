use super::lattice::{
    GCounter, GSet, LamportClock, LwwRegister, ORSet, PNCounter, ReplicaId, VectorClock,
};
use super::config::ConsistencyLevel;
use crate::redis::SDS;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// CrdtValue - Union type for all supported CRDT types
// ============================================================================

/// Enum representing all supported CRDT value types.
/// Each variant wraps a specific CRDT implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrdtValue {
    /// Last-Writer-Wins register for simple key-value storage
    Lww(LwwRegister<SDS>),
    /// Grow-only counter
    GCounter(GCounter),
    /// Positive-Negative counter (supports decrement)
    PNCounter(PNCounter),
    /// Grow-only set
    GSet(GSet<String>),
    /// Observed-Remove set (supports remove)
    ORSet(ORSet<String>),
    /// Hash map with per-field LWW semantics
    Hash(HashMap<String, LwwRegister<SDS>>),
}

impl CrdtValue {
    /// Create a new LWW value
    pub fn new_lww(replica_id: ReplicaId) -> Self {
        CrdtValue::Lww(LwwRegister::new(replica_id))
    }

    /// Create a new GCounter
    pub fn new_gcounter() -> Self {
        CrdtValue::GCounter(GCounter::new())
    }

    /// Create a new PNCounter
    pub fn new_pncounter() -> Self {
        CrdtValue::PNCounter(PNCounter::new())
    }

    /// Create a new GSet
    pub fn new_gset() -> Self {
        CrdtValue::GSet(GSet::new())
    }

    /// Create a new ORSet
    pub fn new_orset() -> Self {
        CrdtValue::ORSet(ORSet::new())
    }

    /// Create a new Hash
    pub fn new_hash() -> Self {
        CrdtValue::Hash(HashMap::new())
    }

    /// Merge two CrdtValues of the same type.
    /// If types don't match, returns self (type mismatch is an error condition).
    pub fn merge(&self, other: &Self) -> Self {
        match (self, other) {
            (CrdtValue::Lww(a), CrdtValue::Lww(b)) => CrdtValue::Lww(a.merge(b)),
            (CrdtValue::GCounter(a), CrdtValue::GCounter(b)) => CrdtValue::GCounter(a.merge(b)),
            (CrdtValue::PNCounter(a), CrdtValue::PNCounter(b)) => CrdtValue::PNCounter(a.merge(b)),
            (CrdtValue::GSet(a), CrdtValue::GSet(b)) => CrdtValue::GSet(a.merge(b)),
            (CrdtValue::ORSet(a), CrdtValue::ORSet(b)) => CrdtValue::ORSet(a.merge(b)),
            (CrdtValue::Hash(a), CrdtValue::Hash(b)) => {
                // Merge each field using LWW semantics
                let mut merged = a.clone();
                for (field, b_lww) in b {
                    merged.entry(field.clone())
                        .and_modify(|a_lww| *a_lww = a_lww.merge(b_lww))
                        .or_insert_with(|| b_lww.clone());
                }
                CrdtValue::Hash(merged)
            }
            // Type mismatch: keep self (this shouldn't happen in normal operation)
            _ => self.clone(),
        }
    }

    /// Get the type name as a string (for debugging/INFO command)
    pub fn type_name(&self) -> &'static str {
        match self {
            CrdtValue::Lww(_) => "lww",
            CrdtValue::GCounter(_) => "gcounter",
            CrdtValue::PNCounter(_) => "pncounter",
            CrdtValue::GSet(_) => "gset",
            CrdtValue::ORSet(_) => "orset",
            CrdtValue::Hash(_) => "hash",
        }
    }

    /// Check if this is an LWW value
    pub fn is_lww(&self) -> bool {
        matches!(self, CrdtValue::Lww(_))
    }

    /// Get as LWW register (for backward compatibility)
    pub fn as_lww(&self) -> Option<&LwwRegister<SDS>> {
        match self {
            CrdtValue::Lww(lww) => Some(lww),
            _ => None,
        }
    }

    /// Get as mutable LWW register
    pub fn as_lww_mut(&mut self) -> Option<&mut LwwRegister<SDS>> {
        match self {
            CrdtValue::Lww(lww) => Some(lww),
            _ => None,
        }
    }

    /// Get as GCounter
    pub fn as_gcounter(&self) -> Option<&GCounter> {
        match self {
            CrdtValue::GCounter(gc) => Some(gc),
            _ => None,
        }
    }

    /// Get as mutable GCounter
    pub fn as_gcounter_mut(&mut self) -> Option<&mut GCounter> {
        match self {
            CrdtValue::GCounter(gc) => Some(gc),
            _ => None,
        }
    }

    /// Get as PNCounter
    pub fn as_pncounter(&self) -> Option<&PNCounter> {
        match self {
            CrdtValue::PNCounter(pn) => Some(pn),
            _ => None,
        }
    }

    /// Get as mutable PNCounter
    pub fn as_pncounter_mut(&mut self) -> Option<&mut PNCounter> {
        match self {
            CrdtValue::PNCounter(pn) => Some(pn),
            _ => None,
        }
    }

    /// Get as GSet
    pub fn as_gset(&self) -> Option<&GSet<String>> {
        match self {
            CrdtValue::GSet(gs) => Some(gs),
            _ => None,
        }
    }

    /// Get as mutable GSet
    pub fn as_gset_mut(&mut self) -> Option<&mut GSet<String>> {
        match self {
            CrdtValue::GSet(gs) => Some(gs),
            _ => None,
        }
    }

    /// Get as ORSet
    pub fn as_orset(&self) -> Option<&ORSet<String>> {
        match self {
            CrdtValue::ORSet(os) => Some(os),
            _ => None,
        }
    }

    /// Get as mutable ORSet
    pub fn as_orset_mut(&mut self) -> Option<&mut ORSet<String>> {
        match self {
            CrdtValue::ORSet(os) => Some(os),
            _ => None,
        }
    }

    /// Get as Hash
    pub fn as_hash(&self) -> Option<&HashMap<String, LwwRegister<SDS>>> {
        match self {
            CrdtValue::Hash(h) => Some(h),
            _ => None,
        }
    }

    /// Get as mutable Hash
    pub fn as_hash_mut(&mut self) -> Option<&mut HashMap<String, LwwRegister<SDS>>> {
        match self {
            CrdtValue::Hash(h) => Some(h),
            _ => None,
        }
    }
}

// ============================================================================
// ReplicatedValue - Wrapper with metadata
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicatedValue {
    /// The underlying CRDT value
    pub crdt: CrdtValue,
    /// Optional vector clock for causal consistency
    pub vector_clock: Option<VectorClock>,
    /// Optional expiry time in milliseconds
    pub expiry_ms: Option<u64>,
    /// Lamport timestamp of last modification (for LWW values)
    pub timestamp: LamportClock,
    /// Optional per-key replication factor override (None = use cluster default)
    /// Used for hot keys that need higher replication
    pub replication_factor: Option<u8>,
}

impl ReplicatedValue {
    /// Create a new ReplicatedValue with an LWW register (default for string keys)
    pub fn new(replica_id: ReplicaId) -> Self {
        ReplicatedValue {
            crdt: CrdtValue::new_lww(replica_id),
            vector_clock: None,
            expiry_ms: None,
            timestamp: LamportClock::new(replica_id),
            replication_factor: None,
        }
    }

    /// Set a custom replication factor for this key (for hot keys)
    pub fn with_replication_factor(mut self, rf: u8) -> Self {
        self.replication_factor = Some(rf);
        self
    }

    /// Get the replication factor, or the provided default if not set
    pub fn get_replication_factor(&self, default_rf: u8) -> u8 {
        self.replication_factor.unwrap_or(default_rf)
    }

    /// Create a new ReplicatedValue with a specific CRDT type
    pub fn with_crdt(crdt: CrdtValue, replica_id: ReplicaId) -> Self {
        ReplicatedValue {
            crdt,
            vector_clock: None,
            expiry_ms: None,
            timestamp: LamportClock::new(replica_id),
            replication_factor: None,
        }
    }

    /// Create a new ReplicatedValue with an LWW value
    pub fn with_value(value: SDS, timestamp: LamportClock) -> Self {
        ReplicatedValue {
            crdt: CrdtValue::Lww(LwwRegister::with_value(value, timestamp)),
            vector_clock: None,
            expiry_ms: None,
            timestamp,
            replication_factor: None,
        }
    }

    /// Set an LWW value (backward compatible)
    pub fn set(&mut self, value: SDS, clock: &mut LamportClock, vc: Option<&mut VectorClock>) {
        // Ensure we have an LWW value
        if let CrdtValue::Lww(ref mut lww) = self.crdt {
            lww.set(value, clock);
        } else {
            // Convert to LWW if type changed
            let mut lww = LwwRegister::new(clock.replica_id);
            lww.set(value, clock);
            self.crdt = CrdtValue::Lww(lww);
        }
        self.timestamp = *clock;
        if let Some(vc) = vc {
            vc.increment(clock.replica_id);
            self.vector_clock = Some(vc.clone());
        }
    }

    /// Delete an LWW value (backward compatible)
    pub fn delete(&mut self, clock: &mut LamportClock) {
        if let CrdtValue::Lww(ref mut lww) = self.crdt {
            lww.delete(clock);
            self.timestamp = *clock;
        }
    }

    /// Merge two ReplicatedValues
    pub fn merge(&self, other: &Self) -> Self {
        let merged_crdt = self.crdt.merge(&other.crdt);
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
        let merged_timestamp = self.timestamp.merge(&other.timestamp);
        // For RF, take the higher value (more replicas = safer)
        let merged_rf = match (self.replication_factor, other.replication_factor) {
            (Some(rf1), Some(rf2)) => Some(rf1.max(rf2)),
            (Some(rf), None) | (None, Some(rf)) => Some(rf),
            (None, None) => None,
        };

        ReplicatedValue {
            crdt: merged_crdt,
            vector_clock: merged_vc,
            expiry_ms: merged_expiry,
            timestamp: merged_timestamp,
            replication_factor: merged_rf,
        }
    }

    /// Get the LWW value (backward compatible)
    pub fn get(&self) -> Option<&SDS> {
        match &self.crdt {
            CrdtValue::Lww(lww) => lww.get(),
            _ => None,
        }
    }

    /// Check if this is a tombstoned LWW value
    pub fn is_tombstone(&self) -> bool {
        match &self.crdt {
            CrdtValue::Lww(lww) => lww.tombstone,
            _ => false,
        }
    }

    /// Get the CRDT type name
    pub fn crdt_type(&self) -> &'static str {
        self.crdt.type_name()
    }

    // ========================================================================
    // CRDT-specific accessors
    // ========================================================================

    /// Get mutable reference to the underlying CRDT
    pub fn crdt_mut(&mut self) -> &mut CrdtValue {
        &mut self.crdt
    }

    /// Convenience: get LWW register reference (backward compatible with `lww` field)
    pub fn lww(&self) -> Option<&LwwRegister<SDS>> {
        self.crdt.as_lww()
    }

    /// Convenience: get mutable LWW register reference
    pub fn lww_mut(&mut self) -> Option<&mut LwwRegister<SDS>> {
        self.crdt.as_lww_mut()
    }

    /// Check if this is a Hash value
    pub fn is_hash(&self) -> bool {
        matches!(self.crdt, CrdtValue::Hash(_))
    }

    /// Get the hash map (if this is a Hash value)
    pub fn get_hash(&self) -> Option<&HashMap<String, LwwRegister<SDS>>> {
        self.crdt.as_hash()
    }

    /// Get mutable hash map (if this is a Hash value)
    pub fn get_hash_mut(&mut self) -> Option<&mut HashMap<String, LwwRegister<SDS>>> {
        self.crdt.as_hash_mut()
    }

    /// Set a field in the hash (creates Hash if needed)
    ///
    /// TigerStyle: Preconditions checked, postconditions verified
    pub fn hash_set(&mut self, field: String, value: SDS, clock: &mut LamportClock) {
        // TigerStyle: Preconditions
        debug_assert!(!field.is_empty(), "Precondition: field must not be empty");

        // Capture for postcondition check
        #[cfg(debug_assertions)]
        let expected_value = value.clone();

        // Ensure we have a Hash value
        if !self.is_hash() {
            self.crdt = CrdtValue::new_hash();
        }

        if let CrdtValue::Hash(ref mut hash) = self.crdt {
            let lww = hash.entry(field.clone()).or_insert_with(|| LwwRegister::new(clock.replica_id));
            lww.set(value, clock);
        }
        self.timestamp = *clock;

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert!(
                self.is_hash(),
                "Postcondition: crdt must be Hash after hash_set"
            );
            debug_assert!(
                self.hash_get(&field).is_some(),
                "Postcondition: field '{}' must exist after hash_set",
                field
            );
            debug_assert_eq!(
                self.hash_get(&field).map(|v| v.as_bytes()),
                Some(expected_value.as_bytes()),
                "Postcondition: field value must match set value"
            );
        }
    }

    /// Delete a field from the hash (marks as tombstone)
    ///
    /// TigerStyle: Preconditions checked, postconditions verified
    pub fn hash_delete(&mut self, field: &str, clock: &mut LamportClock) {
        // TigerStyle: Preconditions
        debug_assert!(!field.is_empty(), "Precondition: field must not be empty");

        if let CrdtValue::Hash(ref mut hash) = self.crdt {
            if let Some(lww) = hash.get_mut(field) {
                lww.delete(clock);
            }
        }
        self.timestamp = *clock;

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            // After delete, field should return None (tombstoned)
            debug_assert!(
                self.hash_get(field).is_none(),
                "Postcondition: field '{}' must be tombstoned after hash_delete",
                field
            );
        }
    }

    /// Get a field from the hash
    pub fn hash_get(&self, field: &str) -> Option<&SDS> {
        // TigerStyle: Precondition
        debug_assert!(!field.is_empty(), "Precondition: field must not be empty");

        if let CrdtValue::Hash(ref hash) = self.crdt {
            hash.get(field).and_then(|lww| lww.get())
        } else {
            None
        }
    }

    /// TigerStyle: Verify hash invariants
    #[cfg(debug_assertions)]
    pub fn verify_hash_invariants(&self) {
        if let CrdtValue::Hash(ref hash) = self.crdt {
            // Invariant: All non-tombstoned fields should be retrievable
            for (field, lww) in hash {
                if !lww.tombstone {
                    debug_assert!(
                        lww.get().is_some(),
                        "Invariant: non-tombstoned field '{}' must have value",
                        field
                    );
                }
            }
        }
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

    /// Record a hash field write (HSET)
    ///
    /// TigerStyle: Preconditions checked, postconditions verified
    pub fn record_hash_write(&mut self, key: String, fields: Vec<(String, SDS)>) -> ReplicationDelta {
        // TigerStyle: Preconditions
        debug_assert!(!key.is_empty(), "Precondition: key must not be empty");
        debug_assert!(!fields.is_empty(), "Precondition: fields must not be empty");

        #[cfg(debug_assertions)]
        let pre_pending_len = self.pending_deltas.len();
        #[cfg(debug_assertions)]
        let field_names: Vec<String> = fields.iter().map(|(f, _)| f.clone()).collect();

        let mut replicated = self.replicated_keys
            .remove(&key)
            .unwrap_or_else(|| {
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
                self.replicated_keys.get(&key).map(|v| v.is_hash()).unwrap_or(false),
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
    pub fn record_hash_delete(&mut self, key: String, fields: Vec<String>) -> Option<ReplicationDelta> {
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

    // ==========================================================================
    // Hash CRDT Tests (DST-style with multiple seeds)
    // ==========================================================================

    #[test]
    fn test_hash_write_and_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Write different fields from different replicas
        let delta1 = state1.record_hash_write(
            "myhash".to_string(),
            vec![("field1".to_string(), SDS::from_str("value1"))],
        );
        let delta2 = state2.record_hash_write(
            "myhash".to_string(),
            vec![("field2".to_string(), SDS::from_str("value2"))],
        );

        // Apply cross-replica deltas
        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        // Both replicas should have both fields
        let hash1 = state1.get_replicated("myhash").unwrap().get_hash().unwrap();
        let hash2 = state2.get_replicated("myhash").unwrap().get_hash().unwrap();

        assert_eq!(hash1.len(), 2, "state1 should have 2 fields");
        assert_eq!(hash2.len(), 2, "state2 should have 2 fields");

        // Values should match
        assert_eq!(
            hash1.get("field1").and_then(|lww| lww.get()).map(|s| s.as_bytes()),
            hash2.get("field1").and_then(|lww| lww.get()).map(|s| s.as_bytes()),
            "field1 values should match"
        );
        assert_eq!(
            hash1.get("field2").and_then(|lww| lww.get()).map(|s| s.as_bytes()),
            hash2.get("field2").and_then(|lww| lww.get()).map(|s| s.as_bytes()),
            "field2 values should match"
        );
    }

    #[test]
    fn test_hash_field_conflict_resolution() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Write same field from both replicas (concurrent conflict)
        let delta1 = state1.record_hash_write(
            "myhash".to_string(),
            vec![("field".to_string(), SDS::from_str("value_from_r1"))],
        );
        let delta2 = state2.record_hash_write(
            "myhash".to_string(),
            vec![("field".to_string(), SDS::from_str("value_from_r2"))],
        );

        // Apply cross-replica deltas
        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        // Both replicas should converge to the same value (LWW semantics)
        let val1 = state1.get_replicated("myhash").unwrap().hash_get("field");
        let val2 = state2.get_replicated("myhash").unwrap().hash_get("field");
        assert_eq!(
            val1.map(|s| s.as_bytes()),
            val2.map(|s| s.as_bytes()),
            "Concurrent hash field writes should converge to same value"
        );
    }

    #[test]
    fn test_hash_field_deletion() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Create hash with multiple fields
        let delta1 = state1.record_hash_write(
            "myhash".to_string(),
            vec![
                ("field1".to_string(), SDS::from_str("value1")),
                ("field2".to_string(), SDS::from_str("value2")),
            ],
        );
        state2.apply_remote_delta(delta1.clone());

        // Delete field1 from state1
        let delete_delta = state1.record_hash_delete(
            "myhash".to_string(),
            vec!["field1".to_string()],
        );

        // Apply deletion to state2
        if let Some(d) = delete_delta {
            state2.apply_remote_delta(d);
        }

        // field1 should be tombstoned, field2 should remain
        let hash1 = state1.get_replicated("myhash").unwrap().get_hash().unwrap();
        let hash2 = state2.get_replicated("myhash").unwrap().get_hash().unwrap();

        // field1 should be tombstoned (value None or tombstone=true)
        let field1_lww_1 = hash1.get("field1").unwrap();
        let field1_lww_2 = hash2.get("field1").unwrap();
        assert!(field1_lww_1.tombstone, "field1 should be tombstoned in state1");
        assert!(field1_lww_2.tombstone, "field1 should be tombstoned in state2");

        // field2 should still have value
        assert_eq!(
            hash1.get("field2").and_then(|lww| lww.get()).map(|s| s.as_bytes()),
            Some(b"value2".as_slice()),
            "field2 should still have value in state1"
        );
        assert_eq!(
            hash2.get("field2").and_then(|lww| lww.get()).map(|s| s.as_bytes()),
            Some(b"value2".as_slice()),
            "field2 should still have value in state2"
        );
    }

    #[test]
    fn test_hash_delete_wins_over_concurrent_write() {
        // Test that delete-after-write wins in LWW semantics
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // r1 writes a field
        let write_delta = state1.record_hash_write(
            "myhash".to_string(),
            vec![("field".to_string(), SDS::from_str("value"))],
        );
        state2.apply_remote_delta(write_delta.clone());

        // r2 deletes the same field (happens after write due to clock)
        let delete_delta = state2.record_hash_delete(
            "myhash".to_string(),
            vec!["field".to_string()],
        );

        // Apply delete to r1
        if let Some(d) = delete_delta {
            state1.apply_remote_delta(d);
        }

        // Both should have tombstoned field
        let hash1 = state1.get_replicated("myhash").unwrap().get_hash().unwrap();
        let hash2 = state2.get_replicated("myhash").unwrap().get_hash().unwrap();

        assert!(hash1.get("field").unwrap().tombstone, "field should be tombstoned in state1");
        assert!(hash2.get("field").unwrap().tombstone, "field should be tombstoned in state2");
    }

    /// DST-style multi-seed test for hash convergence
    #[test]
    fn test_hash_convergence_multi_seed() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        for seed in 0..50 {
            let mut hasher = DefaultHasher::new();
            seed.hash(&mut hasher);
            let hash_seed = hasher.finish();

            // Create 3 replicas
            let r1 = ReplicaId::new(1);
            let r2 = ReplicaId::new(2);
            let r3 = ReplicaId::new(3);

            let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
            let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);
            let mut state3 = ShardReplicaState::new(r3, ConsistencyLevel::Eventual);

            // Generate operations based on seed
            let num_ops = 10 + (hash_seed % 20) as usize;
            let mut deltas = Vec::new();

            for i in 0..num_ops {
                let op_type = (hash_seed.wrapping_add(i as u64)) % 3;
                let field = format!("field{}", (hash_seed.wrapping_add(i as u64)) % 5);
                let value = format!("value_{}_seed{}", i, seed);

                let delta = match op_type {
                    0 => state1.record_hash_write(
                        "hash".to_string(),
                        vec![(field, SDS::from_str(&value))],
                    ),
                    1 => state2.record_hash_write(
                        "hash".to_string(),
                        vec![(field, SDS::from_str(&value))],
                    ),
                    _ => state3.record_hash_write(
                        "hash".to_string(),
                        vec![(field, SDS::from_str(&value))],
                    ),
                };
                deltas.push(delta);
            }

            // Apply all deltas to all replicas
            for delta in &deltas {
                state1.apply_remote_delta(delta.clone());
                state2.apply_remote_delta(delta.clone());
                state3.apply_remote_delta(delta.clone());
            }

            // All replicas should converge
            let hash1 = state1.get_replicated("hash").map(|v| v.get_hash());
            let hash2 = state2.get_replicated("hash").map(|v| v.get_hash());
            let hash3 = state3.get_replicated("hash").map(|v| v.get_hash());

            // Verify all replicas have same field values
            if let (Some(Some(h1)), Some(Some(h2)), Some(Some(h3))) = (hash1, hash2, hash3) {
                assert_eq!(h1.len(), h2.len(), "seed {}: hash1 and hash2 should have same field count", seed);
                assert_eq!(h2.len(), h3.len(), "seed {}: hash2 and hash3 should have same field count", seed);

                for (field, lww1) in h1.iter() {
                    let lww2 = h2.get(field).expect(&format!("seed {}: field {} missing in hash2", seed, field));
                    let lww3 = h3.get(field).expect(&format!("seed {}: field {} missing in hash3", seed, field));

                    assert_eq!(
                        lww1.get().map(|s| s.as_bytes()),
                        lww2.get().map(|s| s.as_bytes()),
                        "seed {}: field {} value mismatch between hash1 and hash2",
                        seed, field
                    );
                    assert_eq!(
                        lww2.get().map(|s| s.as_bytes()),
                        lww3.get().map(|s| s.as_bytes()),
                        "seed {}: field {} value mismatch between hash2 and hash3",
                        seed, field
                    );
                }
            }
        }
    }

    /// Test hash invariants are maintained
    #[test]
    fn test_hash_invariants() {
        let r1 = ReplicaId::new(1);
        let mut state = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);

        // Create a hash and verify invariants
        state.record_hash_write(
            "test_hash".to_string(),
            vec![
                ("a".to_string(), SDS::from_str("1")),
                ("b".to_string(), SDS::from_str("2")),
                ("c".to_string(), SDS::from_str("3")),
            ],
        );

        let replicated = state.get_replicated("test_hash").unwrap();
        assert!(replicated.is_hash(), "Value should be hash type");

        // Verify the hash-specific invariants
        #[cfg(debug_assertions)]
        replicated.verify_hash_invariants();

        // Delete a field and verify invariants still hold
        state.record_hash_delete("test_hash".to_string(), vec!["b".to_string()]);

        let replicated = state.get_replicated("test_hash").unwrap();
        #[cfg(debug_assertions)]
        replicated.verify_hash_invariants();
    }
}
