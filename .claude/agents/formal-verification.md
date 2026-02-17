---
name: formal-verification
description: TLA+ specs, Stateright model checking, Kani proofs, and Maelstrom integration
user_invocable: true
---

# Formal Verification — redis-rust

You are about to work on specifications, model checking, or CRDT proofs.

The verification tools used here (TLA+, model checking, bounded verification, Jepsen-style
testing) are established techniques from Lamport, Newcombe et al. ("Use of Formal Methods
at Amazon Web Services", 2015), and Kingsbury (Jepsen). The concepts are general; the
models and proofs below are project-specific.

---

## 1. Verification Pyramid

```
       ┌───────────────────────────────┐
       │       TLA+ / P Specs          │  ← Protocol-level correctness
       │  (Formal Protocol Models)     │     5 specs in specs/tla/
       └───────────────────────────────┘
                     ↓
       ┌───────────────────────────────┐
       │    Shared Invariants Layer    │  ← Code-level invariants
       │ (invariants/*.rs - code refs) │
       └───────────────────────────────┘
                     ↓
    ┌──────────────────────────────────────┐
    │   Stateright  │   DST Tests  │ Kani │  ← Implementation-level
    │  (Exhaustive) │ (Simulation) │(Proof)│     verification
    └──────────────────────────────────────┘
                     ↓
    ┌──────────────────────────────────────┐
    │           Maelstrom                  │  ← Distributed system testing
    │  (Jepsen-style linearizability)      │     under real partitions
    └──────────────────────────────────────┘
```

Each layer catches different classes of bugs:
- **TLA+**: Protocol design bugs (e.g., "does gossip converge?")
- **Stateright**: Exhaustive state-space bugs (e.g., "does merge violate commutativity?")
- **Kani**: Bounded proof bugs (e.g., "does LWW merge work for ALL inputs?")
- **DST**: Runtime bugs under fault injection (e.g., "does it crash under partition?")
- **Maelstrom**: Distributed system bugs (e.g., "is it linearizable under partitions?")

---

## 2. TLA+ Specifications

**Directory:** `specs/tla/`

### Available Specs

| Spec | File | What it models |
|------|------|---------------|
| Gossip Protocol | `specs/tla/GossipProtocol.tla` | Message dissemination, convergence under partitions |
| Replication Convergence | `specs/tla/ReplicationConvergence.tla` | CRDT merge properties, eventual consistency |
| Anti-Entropy | `specs/tla/AntiEntropy.tla` | Merkle tree sync, divergence detection, partition healing |
| Streaming Persistence | `specs/tla/StreamingPersistence.tla` | Write buffer bounds, segment durability, crash recovery |
| WAL Durability | `specs/tla/WalDurability.tla` | WAL group commit, fsync policies, truncation safety, crash recovery with object store high-water mark |

### Invariant-to-Code Mapping

| TLA+ Invariant | Stateright Property | Code Location |
|---------------|--------------------|----|
| `CRDT_MERGE_COMMUTATIVE` | `CrdtMergeModel` properties | `src/stateright/replication.rs` |
| `WRITE_BUFFER_BOUNDED` | `WriteBufferModel::write_buffer_bounded` | `src/stateright/persistence.rs` |
| `SYNC_COMPLETENESS` | `AntiEntropyModel::sync_convergence_progress` | `src/stateright/anti_entropy.rs` |
| `SEGMENT_ID_MONOTONIC` | `WriteBufferModel::segment_id_monotonic` | `src/stateright/persistence.rs` |
| `MERKLE_CONSISTENCY` | `AntiEntropyModel` properties | `src/stateright/anti_entropy.rs` |
| `WAL_DURABILITY` | `WalDurabilityModel::truncation_safety` | `src/stateright/persistence.rs` |
| `TRUNCATION_SAFETY` | `WalDurabilityModel::truncation_safety` | `src/stateright/persistence.rs` |
| `RECOVERY_COMPLETENESS` | `WalDurabilityModel::recovery_completeness` | `src/stateright/persistence.rs` |
| `GROUP_COMMIT_ATOMICITY` | `WalDurabilityModel::buffer_not_acknowledged` | `src/stateright/persistence.rs` |

### When to Update TLA+ Specs

- Changing the gossip protocol → update `GossipProtocol.tla`
- Changing CRDT merge semantics → update `ReplicationConvergence.tla`
- Changing anti-entropy sync → update `AntiEntropy.tla`
- Changing write buffer/persistence → update `StreamingPersistence.tla`
- Changing WAL behavior/group commit/recovery → update `WalDurability.tla`

---

## 3. Stateright Model Checking

**Files:** `src/stateright/mod.rs`, `replication.rs`, `persistence.rs`, `anti_entropy.rs`

Stateright exhaustively explores every reachable state from an initial state. It
verifies that properties hold in ALL states (not just sampled states like DST).

### Available Models

#### CrdtMergeModel (`src/stateright/replication.rs`)

Verifies CRDT merge properties for LWW registers:

```rust
pub struct CrdtMergeModel {
    pub replica_ids: Vec<ReplicaId>,  // Default: [1, 2]
    pub keys: Vec<u64>,              // Default: [1]
    pub values: Vec<u64>,            // Default: [10, 20]
    pub max_clock: u64,              // Default: 3
}
```

**Properties verified:**
- `lamport_monotonic` — Lamport clocks never exceed max
- `tombstone_consistency` — Tombstoned registers have no value
- `valid_timestamps` — All timestamps within bounds

**Companion functions:**
- `verify_merge_commutative(a, b)` — merge(a, b) = merge(b, a)
- `verify_merge_associative(a, b, c)` — merge(a, merge(b, c)) = merge(merge(a, b), c)
- `verify_merge_idempotent(a)` — merge(a, a) = a

**Actions:** `Set { replica, key, value }`, `Delete { replica, key }`, `Sync { from, to, key }`

#### WriteBufferModel (`src/stateright/persistence.rs`)

Verifies streaming persistence properties:

```rust
pub struct WriteBufferModel {
    pub config: WriteBufferConfig,  // max_buffer_size, backpressure_threshold, etc.
    pub keys: Vec<u64>,
    pub values: Vec<u64>,
}
```

**Properties verified (6):**
- `write_buffer_bounded` — Buffer never exceeds backpressure threshold
- `segment_id_monotonic` — Segment IDs always increase
- `manifest_consistent` — Manifest only references written segments
- `buffer_size_consistent` — Buffer size matches actual delta sizes
- `recovered_state_valid` — Recovery produces valid state
- `no_segment_id_reuse` — Segment IDs are never reused

**Actions:** `PushDelta { key, value }`, `Flush`, `Crash`, `Recover`

#### WalDurabilityModel (`src/stateright/persistence.rs`)

Verifies WAL + streaming hybrid persistence durability invariants:

```rust
pub struct WalDurabilityModel {
    pub config: WalDurabilityConfig,  // max_writes, group_commit_batch_size, etc.
}
```

**Properties verified (5):**
- `truncation_safety` — Every acknowledged entry is in wal_synced OR streamed
- `recovery_completeness` — After recovery, all acknowledged entries are recoverable
- `high_water_mark_consistent` — High-water mark matches max streamed timestamp
- `buffer_not_acknowledged` — Entries in WAL buffer have not been acknowledged (pre-sync)
- `acknowledged_recoverable` — All acknowledged entries are in WAL or object store

**Actions:** `WalAppend`, `WalSync`, `WalSyncFail`, `StreamFlush`, `WalTruncate`, `Crash`, `Recover`

**Run:**
```bash
cargo test -p redis-sim stateright_wal_durability -- --ignored --nocapture
```

#### AntiEntropyModel (`src/stateright/anti_entropy.rs`)

Verifies Merkle tree-based synchronization:

```rust
pub struct AntiEntropyModel {
    pub replica_ids: Vec<ReplicaId>,  // Default: [1, 2, 3]
    pub keys: Vec<u64>,              // Default: [1, 2]
    pub values: Vec<u64>,            // Default: [10, 20, 30]
    pub max_generation: u64,         // Default: 5
}
```

**Properties verified (5):**
- `generation_monotonic` — Generation counters bounded
- `no_self_divergence` — No replica marks itself as divergent
- `partition_symmetric` — Partition representation is consistent
- `sync_requests_valid` — Sync requests reference valid replicas
- `sync_convergence_progress` — State is well-formed after sync

**Actions:** `LocalWrite`, `ExchangeDigest`, `InitiateSync`, `CompleteSync`, `CreatePartition`, `HealPartition`

### Running Stateright Tests

```bash
# All model checks (marked #[ignore] for CI speed)
cargo test -p redis-sim stateright -- --ignored --nocapture

# Specific model
cargo test -p redis-sim stateright_replication -- --ignored --nocapture
cargo test -p redis-sim stateright_persistence -- --ignored --nocapture
cargo test -p redis-sim stateright_anti_entropy -- --ignored --nocapture
cargo test -p redis-sim stateright_wal_durability -- --ignored --nocapture
```

### How to Add a New Stateright Model

1. Create `src/stateright/your_model.rs`
2. Define `State`, `Action`, and model struct
3. Implement `stateright::Model` trait:
   - `init_states()` — starting states
   - `actions(state, actions)` — available actions from a state
   - `next_state(state, action)` — transition function
   - `properties()` — invariants to check
4. Add `pub mod your_model;` to `src/stateright/mod.rs`
5. Add `#[test] #[ignore]` test that runs `model.checker().spawn_bfs().join()`
6. Keep state space small (2-3 replicas, 1-2 keys) for tractable exploration
7. Map invariants to TLA+ spec if one exists

---

## 4. Kani Bounded Proofs

**File:** `src/replication/lattice.rs` (inside `#[cfg(kani)] mod kani_proofs`)

Kani uses bounded model checking to prove properties for ALL inputs within bounds.
Unlike testing (which samples), Kani explores every possible input combination.

### Available Proofs (7)

| Proof | What it verifies |
|-------|-----------------|
| `verify_lww_merge_commutative` | `merge(a, b) = merge(b, a)` for all LWW register values |
| `verify_lww_merge_idempotent` | `merge(a, a) = a` for all LWW register values |
| `verify_lamport_clock_total_order` | Lamport clocks form a total order |
| `verify_gcounter_merge_commutative` | GCounter merge is commutative |
| `verify_gcounter_merge_idempotent` | GCounter merge is idempotent |
| `verify_pncounter_merge_commutative` | PNCounter merge is commutative |
| `verify_vector_clock_merge_commutative` | VectorClock merge is commutative |

### Proof Pattern

```rust
#[kani::proof]
#[kani::unwind(5)]
fn verify_lww_merge_commutative() {
    let r1 = ReplicaId::new(kani::any());
    let r2 = ReplicaId::new(kani::any());

    let a: LwwRegister<u64> = LwwRegister {
        value: kani::any(),
        timestamp: LamportClock { time: kani::any(), replica_id: r1 },
        tombstone: kani::any(),
    };

    let b: LwwRegister<u64> = LwwRegister {
        value: kani::any(),
        timestamp: LamportClock { time: kani::any(), replica_id: r2 },
        tombstone: kani::any(),
    };

    let ab = a.merge(&b);
    let ba = b.merge(&a);
    kani::assert(ab == ba, "LWW merge must be commutative");
}
```

### How to Add a New Kani Proof

1. Open `src/replication/lattice.rs`
2. Add proof inside `#[cfg(kani)] mod kani_proofs`
3. Use `kani::any()` for symbolic inputs
4. Use `#[kani::unwind(N)]` to bound loop unrolling
5. Use `kani::assert(condition, "message")` for the property
6. Keep bounds small — Kani explores exponentially

### Running Kani Proofs

**Note:** Kani requires the kani toolchain to be installed (currently commented out in
`Cargo.toml`):

```bash
# Install kani
cargo install kani-verifier
kani setup

# Run all proofs
cargo kani --tests

# Run specific proof
cargo kani --harness verify_lww_merge_commutative
```

---

## 5. Maelstrom Integration

**Files:**
- `src/bin/maelstrom_kv.rs` — Single-node KV store
- `src/bin/maelstrom_kv_replicated.rs` — Replicated KV store

Maelstrom is a Jepsen-style workload generator that tests distributed systems by
injecting real network partitions and checking linearizability.

### What it proves

- **Gossip correctness**: Messages eventually delivered to all nodes
- **Eventual consistency**: All replicas converge after partitions heal
- **Basic liveness**: System continues accepting requests during partitions

### What it does NOT prove

- **Linearizability**: Our AP system is eventually consistent, not linearizable
- **Strong consistency**: By design — we chose AP over CP

### Running Maelstrom

Requires Java 11+:

```bash
# Install Maelstrom
wget https://github.com/jepsen-io/maelstrom/releases/download/v0.2.3/maelstrom.tar.bz2
tar xjf maelstrom.tar.bz2

# Build the binary
cargo build --release --bin maelstrom-kv-replicated

# Run linearizability test (will find violations — expected for AP system)
./maelstrom/maelstrom test -w lin-kv \
    --bin ./target/release/maelstrom-kv-replicated \
    --node-count 3 --time-limit 60 --rate 100

# Run with network partitions
./maelstrom/maelstrom test -w lin-kv \
    --bin ./target/release/maelstrom-kv-replicated \
    --node-count 5 --time-limit 120 --rate 50 \
    --nemesis partition
```

### Maelstrom Binary Structure

The `maelstrom_kv_replicated.rs` binary:
- Uses `ReplicatedShardedState` from `redis_sim::production`
- Handles Maelstrom JSON protocol messages
- Uses `LamportClock`, `ReplicatedValue`, `ReplicationDelta` from replication module
- Implements distributed key-value operations with CRDT merging

---

## 6. Cross-Layer Consistency

When modifying a protocol or algorithm, update ALL relevant layers:

| Change | TLA+ | Stateright | Kani | DST | Maelstrom |
|--------|------|-----------|------|-----|-----------|
| New CRDT type | - | Add model | Add proofs | Add DST harness | - |
| Change merge semantics | Update spec | Update model | Update proofs | Update shadow | Retest |
| Change gossip protocol | Update GossipProtocol.tla | - | - | - | Retest |
| Change anti-entropy | Update AntiEntropy.tla | Update model | - | - | Retest |
| Change persistence | Update StreamingPersistence.tla | Update model | - | Update DST | - |
| Change WAL / durability | Update WalDurability.tla | Update model | - | Update wal_dst | - |

---

## 7. Adding a New CRDT (Full Verification Stack)

1. **Define type** in `src/replication/lattice.rs`
   - Implement `merge()` method

2. **Add Kani proofs** in `#[cfg(kani)] mod kani_proofs`:
   - `verify_<type>_merge_commutative`
   - `verify_<type>_merge_idempotent`
   - (Optional) `verify_<type>_merge_associative`

3. **Add Stateright model** if architectural properties need exhaustive checking:
   - Create model in `src/stateright/`
   - Map to TLA+ invariants

4. **Add DST harness** in `src/replication/crdt_dst.rs`:
   - Shadow state comparison
   - Fault injection with `FaultConfig::moderate()`
   - Multiple seeds (minimum 10)

5. **Update TLA+ spec** if protocol-level behavior changes:
   - Spec in `specs/tla/ReplicationConvergence.tla`

6. **Test with Maelstrom** if the CRDT is used in replication:
   - Rebuild `maelstrom-kv-replicated` binary
   - Run Jepsen-style tests

---

## Anti-patterns

- **Skipping a verification layer.** Each layer catches different bugs. Don't skip Kani because "Stateright already checked it" — they verify different properties at different abstraction levels.
- **Large state spaces in Stateright.** Keep models small (2-3 replicas, 1-2 keys). Stateright explores exponentially. Use DST for larger configurations.
- **Forgetting to map TLA+ to code.** Every TLA+ invariant should have a corresponding Stateright property or DST assertion. Document the mapping.
- **Testing CRDTs without symbolic inputs.** Kani with `kani::any()` proves properties for ALL inputs. Unit tests with specific values only cover those values.
- **Expecting linearizability from Maelstrom.** Our AP system is eventually consistent. Maelstrom lin-kv tests WILL find violations — that's expected. Use Maelstrom to verify liveness and convergence, not strict linearizability.
