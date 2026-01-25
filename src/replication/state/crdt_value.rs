//! CrdtValue - Union type for all supported CRDT types

use super::error::CrdtTypeMismatchError;
use crate::redis::SDS;
use crate::replication::lattice::{
    GCounter, GSet, LamportClock, LwwRegister, ORSet, PNCounter, ReplicaId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

    /// Try to merge two CrdtValues of the same type.
    /// Returns an error if types don't match - this makes type conflicts explicit
    /// rather than silently discarding data (TigerStyle: explicit error handling).
    ///
    /// # Errors
    /// Returns `CrdtTypeMismatchError` if self and other have different CRDT types.
    pub fn try_merge(&self, other: &Self) -> Result<Self, CrdtTypeMismatchError> {
        match (self, other) {
            (CrdtValue::Lww(a), CrdtValue::Lww(b)) => Ok(CrdtValue::Lww(a.merge(b))),
            (CrdtValue::GCounter(a), CrdtValue::GCounter(b)) => Ok(CrdtValue::GCounter(a.merge(b))),
            (CrdtValue::PNCounter(a), CrdtValue::PNCounter(b)) => {
                Ok(CrdtValue::PNCounter(a.merge(b)))
            }
            (CrdtValue::GSet(a), CrdtValue::GSet(b)) => Ok(CrdtValue::GSet(a.merge(b))),
            (CrdtValue::ORSet(a), CrdtValue::ORSet(b)) => Ok(CrdtValue::ORSet(a.merge(b))),
            (CrdtValue::Hash(a), CrdtValue::Hash(b)) => {
                // Merge each field using LWW semantics
                let mut merged = a.clone();
                for (field, b_lww) in b {
                    merged
                        .entry(field.clone())
                        .and_modify(|a_lww| *a_lww = a_lww.merge(b_lww))
                        .or_insert_with(|| b_lww.clone());
                }
                Ok(CrdtValue::Hash(merged))
            }
            // Type mismatch: return explicit error instead of silently discarding data
            _ => Err(CrdtTypeMismatchError {
                self_type: self.type_name(),
                other_type: other.type_name(),
            }),
        }
    }

    /// Merge two CrdtValues, using timestamp-based conflict resolution for type mismatches.
    ///
    /// When types match: performs standard CRDT merge.
    /// When types don't match: uses the value with the later timestamp (LWW semantics)
    /// to avoid silent data loss.
    ///
    /// This is the safe default - use `try_merge` if you need to handle type conflicts
    /// explicitly in application logic.
    pub fn merge_with_timestamps(
        &self,
        other: &Self,
        self_timestamp: &LamportClock,
        other_timestamp: &LamportClock,
    ) -> Self {
        match self.try_merge(other) {
            Ok(merged) => merged,
            Err(err) => {
                // Type mismatch: use LWW semantics based on timestamp to avoid data loss
                // Log the conflict for debugging (in debug builds)
                #[cfg(debug_assertions)]
                tracing::warn!(
                    "CRDT type conflict during merge: {} vs {} - using LWW resolution",
                    err.self_type,
                    err.other_type
                );

                // Keep the value with the later timestamp (deterministic resolution)
                if other_timestamp > self_timestamp {
                    other.clone()
                } else {
                    self.clone()
                }
            }
        }
    }

    /// Merge two CrdtValues of the same type (legacy API).
    ///
    /// # Warning
    /// This method logs an error and keeps `self` on type mismatch.
    /// Prefer `try_merge` for explicit error handling or `merge_with_timestamps`
    /// for automatic LWW conflict resolution.
    #[deprecated(
        since = "0.2.0",
        note = "Use try_merge() for explicit error handling or merge_with_timestamps() for safe LWW resolution"
    )]
    pub fn merge(&self, other: &Self) -> Self {
        match self.try_merge(other) {
            Ok(merged) => merged,
            Err(err) => {
                // Log the error - this is a bug in the application logic
                tracing::error!(
                    "CRDT type mismatch during merge: {} vs {} - this indicates a bug! \
                     Keeping self value, but data from other is LOST. \
                     Use merge_with_timestamps() for safe conflict resolution.",
                    err.self_type,
                    err.other_type
                );
                // Keep self to maintain backward compatibility, but this is wrong
                self.clone()
            }
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
