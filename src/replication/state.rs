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

    /// Merge two CrdtValues of the same type.
    /// If types don't match, returns self (type mismatch is an error condition).
    pub fn merge(&self, other: &Self) -> Self {
        match (self, other) {
            (CrdtValue::Lww(a), CrdtValue::Lww(b)) => CrdtValue::Lww(a.merge(b)),
            (CrdtValue::GCounter(a), CrdtValue::GCounter(b)) => CrdtValue::GCounter(a.merge(b)),
            (CrdtValue::PNCounter(a), CrdtValue::PNCounter(b)) => CrdtValue::PNCounter(a.merge(b)),
            (CrdtValue::GSet(a), CrdtValue::GSet(b)) => CrdtValue::GSet(a.merge(b)),
            (CrdtValue::ORSet(a), CrdtValue::ORSet(b)) => CrdtValue::ORSet(a.merge(b)),
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
}
