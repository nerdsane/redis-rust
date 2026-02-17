//! Stateright Model Checking for redis-rust
//!
//! This module provides exhaustive state-space exploration using Stateright,
//! a model checker for distributed systems written in Rust.
//!
//! ## Verification Pyramid Context
//!
//! Stateright models sit in the middle layer of the verification pyramid:
//!
//! ```text
//!        ┌───────────────────────────────┐
//!        │       TLA+ / P Specs          │
//!        │  (Formal Protocol Models)     │
//!        └───────────────────────────────┘
//!                      ↓
//!        ┌───────────────────────────────┐
//!        │    Shared Invariants Layer    │
//!        │ (invariants/*.rs - code refs) │
//!        └───────────────────────────────┘
//!                      ↓
//!     ┌──────────────────────────────────────┐
//!     │   Stateright  │   DST Tests  │ Kani │  ← YOU ARE HERE
//!     │  (Exhaustive) │ (Simulation) │(Proof)│
//!     └──────────────────────────────────────┘
//! ```
//!
//! ## Available Models
//!
//! - `replication`: CRDT merge properties (commutativity, associativity, idempotence)
//! - `persistence`: Write buffer bounds and durability
//! - `anti_entropy`: Merkle tree sync completeness
//!
//! ## Running Model Checks
//!
//! ```bash
//! # Run all Stateright tests (marked #[ignore] for CI speed)
//! cargo test -p redis-sim stateright -- --ignored --nocapture
//!
//! # Run specific model
//! cargo test -p redis-sim stateright_replication -- --ignored --nocapture
//! ```
//!
//! ## Model-to-TLA+ Mapping
//!
//! | Stateright Model | TLA+ Spec | Key Invariants |
//! |------------------|-----------|----------------|
//! | `CrdtMergeModel` | `ReplicationConvergence.tla` | CRDT_MERGE_COMMUTATIVE |
//! | `WriteBufferModel` | `StreamingPersistence.tla` | WRITE_BUFFER_BOUNDED |
//! | `AntiEntropyModel` | `AntiEntropy.tla` | SYNC_COMPLETENESS |

pub mod anti_entropy;
pub mod persistence;
pub mod replication;

#[cfg(test)]
pub use anti_entropy::AntiEntropyModel;
#[cfg(test)]
pub use persistence::{WalDurabilityModel, WriteBufferModel};
#[cfg(test)]
pub use replication::CrdtMergeModel;
