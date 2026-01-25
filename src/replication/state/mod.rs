//! Replication state module - Split for compliance with 500-line limit
//!
//! This module provides CRDT-based replication state management:
//! - `CrdtValue`: Union type for all supported CRDT types
//! - `ReplicatedValue`: Wrapper with metadata (timestamps, expiry)
//! - `ReplicationDelta`: Delta for CRDT replication
//! - `ShardReplicaState`: Per-shard replication state management

mod crdt_value;
mod delta;
mod error;
mod replicated_value;
mod shard_state;

#[cfg(test)]
mod conditional_tests;
#[cfg(test)]
mod hash_tests;
#[cfg(test)]
mod hincrby_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod type_mismatch_tests;

// Re-export all public types
pub use crdt_value::CrdtValue;
pub use delta::ReplicationDelta;
pub use error::CrdtTypeMismatchError;
pub use replicated_value::ReplicatedValue;
pub use shard_state::ShardReplicaState;
