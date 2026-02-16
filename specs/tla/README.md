# TLA+ Specifications for redis-rust

This directory contains formal TLA+ specifications for redis-rust's distributed protocols.

## Verification Pyramid Context

These TLA+ specs sit at the top of the verification pyramid:

```
       ┌───────────────────────────────┐
       │       TLA+ / P Specs          │  ← YOU ARE HERE
       │  (Formal Protocol Models)     │
       └───────────────────────────────┘
                     ↓
       ┌───────────────────────────────┐
       │    Shared Invariants Layer    │
       │ (invariants/*.rs - code refs) │
       └───────────────────────────────┘
                     ↓
    ┌──────────────────────────────────────┐
    │   Stateright  │   DST Tests  │ Kani │
    │  (Exhaustive) │ (Simulation) │(Proof)│
    └──────────────────────────────────────┘
                     ↓
       ┌───────────────────────────────┐
       │    Production Monitoring      │
       │   (Datadog, Bloodhound)       │
       └───────────────────────────────┘
```

## Specifications

| Spec File | Purpose | Key Invariants | Maps To Code |
|-----------|---------|----------------|--------------|
| `ReplicationConvergence.tla` | CRDT-based replication | CRDT_MERGE_COMMUTATIVE, EVENTUAL_CONVERGENCE, LAMPORT_MONOTONIC | `src/replication/lattice.rs`, `state.rs` |
| `GossipProtocol.tla` | Delta dissemination | GOSSIP_DELIVERY, NO_DUPLICATE_PROCESSING, SELECTIVE_ROUTING | `src/replication/gossip.rs`, `gossip_router.rs` |
| `StreamingPersistence.tla` | Write buffer & durability | DURABILITY_GUARANTEE, WRITE_BUFFER_BOUNDED, RECOVERY_COMPLETENESS | `src/streaming/persistence.rs`, `write_buffer.rs` |
| `AntiEntropy.tla` | Partition healing | MERKLE_CONSISTENCY, SYNC_COMPLETENESS, PARTITION_HEALING | `src/replication/anti_entropy.rs` |

## Running Model Checker

### Prerequisites

Install TLC (TLA+ Model Checker):
```bash
# Using tla2tools.jar
wget https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar

# Or via Homebrew (macOS)
brew install tlaplus
```

### Running Specifications

```bash
# Run with default configuration
java -jar tla2tools.jar ReplicationConvergence.tla

# Run with specific model configuration
java -jar tla2tools.jar -config ReplicationConvergence.cfg ReplicationConvergence.tla

# Run with more workers for faster checking
java -jar tla2tools.jar -workers 4 GossipProtocol.tla
```

### Model Configuration Example

Create a `.cfg` file for each spec:

```tla
\* ReplicationConvergence.cfg
CONSTANTS
    Replicas = {r1, r2, r3}
    Keys = {k1, k2}
    Values = {v1, v2}
    MaxTime = 5

INVARIANTS
    TypeOK
    LamportMonotonic
    TombstoneConsistency

PROPERTIES
    EventualConvergence
```

## Invariant Mapping to Code

Each TLA+ invariant has corresponding runtime assertions in the Rust code:

### ReplicationConvergence.tla
| TLA+ Invariant | Rust Implementation |
|----------------|---------------------|
| `CRDT_MERGE_COMMUTATIVE` | `LwwRegister::merge()` in `lattice.rs` - order-independent merge |
| `LAMPORT_MONOTONIC` | `LamportClock::tick()` - `debug_assert!(new_time > old_time)` |
| `EVENTUAL_CONVERGENCE` | DST test: `tests/eventual_consistency_test.rs` |

### GossipProtocol.tla
| TLA+ Invariant | Rust Implementation |
|----------------|---------------------|
| `SOURCE_CORRECT` | `GossipState::verify_invariants()` in `gossip.rs` |
| `NO_DUPLICATE_PROCESSING` | `processed_deltas` set in gossip handler |
| `SELECTIVE_ROUTING` | `GossipRouter::route_deltas()` in `gossip_router.rs` |

### StreamingPersistence.tla
| TLA+ Invariant | Rust Implementation |
|----------------|---------------------|
| `WRITE_BUFFER_BOUNDED` | `WriteBufferError::BackpressureExceeded` check |
| `SEGMENT_ID_MONOTONIC` | `Manifest::allocate_segment_id()` |
| `DURABILITY_GUARANTEE` | DST test: `tests/streaming_dst_test.rs` |

### AntiEntropy.tla
| TLA+ Invariant | Rust Implementation |
|----------------|---------------------|
| `MERKLE_CONSISTENCY` | `StateDigest::from_state()` in `anti_entropy.rs` |
| `PARTITION_HEALING` | `AntiEntropyManager::on_partition_healed()` |
| `SYNC_COMPLETENESS` | `handle_sync_request()` / `handle_sync_response()` |

## Adding New Specifications

When adding a new TLA+ spec:

1. Create the `.tla` file with:
   - Module header with purpose comment
   - CONSTANTS and VARIABLES
   - TypeOK invariant
   - Helper functions
   - Init and Next state relations
   - Safety invariants
   - Liveness properties

2. Update this README with:
   - Entry in the Specifications table
   - Invariant-to-code mapping

3. Create corresponding:
   - Stateright model in `src/stateright/`
   - DST tests that exercise the same scenarios
   - Kani proofs for bounded verification

## Related Documentation

- [ADR-001: Simulation-First Development](../../docs/adr/001-simulation-first-development.md)
- [ADR-004: Anna KVS CRDT Replication](../../docs/adr/004-anna-kvs-crdt-replication.md)
- [EVOLUTION.md](../../docs/adr/EVOLUTION.md) - Architecture characteristics tracking
