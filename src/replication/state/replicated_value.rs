//! ReplicatedValue - Wrapper with metadata for CRDT values

use super::crdt_value::CrdtValue;
use crate::redis::SDS;
use crate::replication::lattice::{LamportClock, LwwRegister, ReplicaId, VectorClock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

    /// Merge two ReplicatedValues.
    ///
    /// Uses `merge_with_timestamps` for CRDT merging to handle type mismatches safely:
    /// - If types match: standard CRDT merge semantics
    /// - If types don't match: LWW resolution based on timestamp (no data loss)
    pub fn merge(&self, other: &Self) -> Self {
        // Use safe merge that handles type conflicts with LWW semantics
        let merged_crdt =
            self.crdt
                .merge_with_timestamps(&other.crdt, &self.timestamp, &other.timestamp);
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
            let lww = hash
                .entry(field.clone())
                .or_insert_with(|| LwwRegister::new(clock.replica_id));
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
