# Stateright Model Checking

This directory contains exhaustive state-space exploration models using [Stateright](https://github.com/stateright/stateright), a model checker for distributed systems.

## Verification Pyramid Context

Stateright models sit in the middle layer of the verification pyramid:

```
       ┌───────────────────────────────┐
       │       TLA+ / P Specs          │
       │  (Formal Protocol Models)     │
       └───────────────────────────────┘
                     ↓
       ┌───────────────────────────────┐
       │    Shared Invariants Layer    │
       │ (invariants/*.rs - code refs) │
       └───────────────────────────────┘
                     ↓
    ┌──────────────────────────────────────┐
    │   Stateright  │   DST Tests  │ Kani │  ← YOU ARE HERE
    │  (Exhaustive) │ (Simulation) │(Proof)│
    └──────────────────────────────────────┘
```

## Available Models

| Model | File | TLA+ Spec | Key Invariants |
|-------|------|-----------|----------------|
| `CrdtMergeModel` | `replication.rs` | `ReplicationConvergence.tla` | CRDT_MERGE_COMMUTATIVE, LAMPORT_MONOTONIC |
| `WriteBufferModel` | `persistence.rs` | `StreamingPersistence.tla` | WRITE_BUFFER_BOUNDED, SEGMENT_ID_MONOTONIC |
| `AntiEntropyModel` | `anti_entropy.rs` | `AntiEntropy.tla` | SYNC_COMPLETENESS, PARTITION_HEALING |

## Running Model Checks

```bash
# Run all Stateright tests (marked #[ignore] for CI speed)
cargo test -p redis-sim stateright -- --ignored --nocapture

# Run specific model
cargo test -p redis-sim stateright_replication -- --ignored --nocapture
cargo test -p redis-sim stateright_persistence -- --ignored --nocapture
cargo test -p redis-sim stateright_anti_entropy -- --ignored --nocapture
```

## Model-to-Code Mapping

### Replication Model (`CrdtMergeModel`)

| Model Concept | Rust Implementation |
|--------------|---------------------|
| `LwwRegister` | `src/replication/lattice.rs::LwwRegister` |
| `CrdtAction::Set` | `LwwRegister::set()` |
| `CrdtAction::Delete` | `LwwRegister::delete()` |
| `CrdtAction::Sync` | `LwwRegister::merge()` |
| `lamport_monotonic` invariant | `LamportClock::tick()` assertions |

### Persistence Model (`WriteBufferModel`)

| Model Concept | Rust Implementation |
|--------------|---------------------|
| `PersistenceState.buffer` | `WriteBuffer::buffer` |
| `PersistenceState.buffer_size` | `WriteBuffer::estimated_bytes` |
| `PersistenceAction::PushDelta` | `WriteBuffer::push()` |
| `PersistenceAction::Flush` | `WriteBuffer::flush()` |
| `BackpressureExceeded` | `WriteBufferError::BackpressureExceeded` |

### Anti-Entropy Model (`AntiEntropyModel`)

| Model Concept | Rust Implementation |
|--------------|---------------------|
| `MerkleDigest` | `StateDigest` in `anti_entropy.rs` |
| `AntiEntropyAction::ExchangeDigest` | `process_peer_digest()` |
| `AntiEntropyAction::CompleteSync` | `handle_sync_response()` |
| `AntiEntropyAction::HealPartition` | `on_partition_healed()` |

## Stateright vs DST

| Aspect | Stateright | DST |
|--------|------------|-----|
| Coverage | Exhaustive (all states) | Statistical (random seeds) |
| State Space | Bounded (model constraints) | Unbounded (time-limited) |
| Speed | Slower (explores all) | Faster (samples) |
| Bugs Found | All within model bounds | Probabilistic |
| Use Case | Protocol correctness | Implementation correctness |

## Adding New Models

1. Create new file in `src/stateright/`
2. Implement `stateright::Model` trait:
   - `State`: Your system state type
   - `Action`: Possible actions
   - `init_states()`: Initial states
   - `actions()`: Generate possible actions from state
   - `next_state()`: Apply action to get new state
   - `properties()`: Invariants to verify

3. Add to `mod.rs`
4. Create corresponding TLA+ spec in `specs/tla/`
5. Map invariants between model and code

## Example: Adding a New Model

```rust
use stateright::{Model, Property};

pub struct MyModel { /* config */ }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MyState { /* state */ }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MyAction { /* actions */ }

impl Model for MyModel {
    type State = MyState;
    type Action = MyAction;

    fn init_states(&self) -> Vec<Self::State> {
        vec![MyState::new()]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        // Generate possible actions
    }

    fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
        // Apply action, return new state
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("my_invariant", |_model, state| {
                // Return true if invariant holds
            }),
        ]
    }
}
```

## References

- [Stateright GitHub](https://github.com/stateright/stateright)
- [TLA+ Specs](../../specs/tla/)
- [ADR-001: Simulation-First Development](../../docs/adr/001-simulation-first-development.md)
