---
name: distributed-systems
description: Domain knowledge for replication, gossip, CRDTs, and multi-node code
user_invocable: true
---

# Distributed Systems — redis-rust

You are about to work on distributed systems code. This skill injects the concepts and
codebase mappings you need.

Sections marked **(General Theory)** are established distributed systems concepts
(Lamport clocks, CRDTs, CAP theorem, etc.) — use your training knowledge as the
primary source. Sections marked **(Project-Specific)** reference our actual code.

---

## 1. Network Failure Modes (General Theory)

Distributed systems must handle:

| Failure | Meaning | Our handling |
|---------|---------|--------------|
| Message loss | Packets silently dropped | Gossip retransmission + anti-entropy |
| Message delay | Unbounded latency | VirtualTime + timeout faults via buggify |
| Message duplication | Same message delivered twice | CRDT merge idempotence guarantees safety |
| Network partition | Subset of nodes unreachable | AP choice — replicas diverge, converge on heal |
| Byzantine fault | Node sends incorrect data | **Not handled** — we assume crash-recovery only |

Buggify faults that simulate these: `network::PACKET_DROP`, `network::DELAY`,
`network::DUPLICATE`, `network::REORDER`, `network::CONNECTION_RESET`,
`replication::GOSSIP_DROP`, `replication::GOSSIP_DELAY`, `replication::SPLIT_BRAIN`.

See `src/buggify/faults.rs` for the full catalog.

---

## 2. Time (General Theory + Project Mapping)

### Lamport Clocks

**File:** `src/replication/lattice.rs`

```rust
pub struct LamportClock {
    pub time: u64,
    pub replica_id: ReplicaId,
}
```

- `tick()` — increment local time
- `update()` — merge with received timestamp (take max + 1)
- `merge()` — take component-wise max
- Total order: compare `(time, replica_id)` — higher time wins, replica_id breaks ties

### Vector Clocks

**File:** `src/replication/lattice.rs`

```rust
pub struct VectorClock {
    clocks: HashMap<ReplicaId, u64>,
}
```

- `increment(replica_id)` — advance own entry
- `merge(other)` — take max per replica
- `happens_before(other)` — true if causally precedes
- `concurrent_with(other)` — true if neither happens-before the other

Vector clocks determine **causality**. Two events are concurrent if neither's vector clock
dominates the other. CRDTs resolve concurrent updates deterministically.

### VirtualTime (simulation only)

**File:** `src/simulator/time.rs`

```rust
pub struct VirtualTime(pub u64);  // millisecond-based
pub struct Duration(pub u64);
```

Production code uses `LamportClock` / `VectorClock`. DST uses `VirtualTime` to control
the simulation clock. Never mix them.

---

## 3. Consistency Models (General Theory + Our Choice)

We chose **AP** (Available + Partition-tolerant) from the CAP theorem:

| Property | Our guarantee |
|----------|---------------|
| Availability | Every non-crashed replica accepts reads and writes |
| Partition tolerance | System continues during network partitions |
| Consistency | **Eventual** — replicas converge after partitions heal |
| Conflict resolution | CRDTs (algebraic, not ad-hoc) |

This means: during a partition, two replicas can accept conflicting writes. CRDTs
guarantee that when the partition heals and replicas sync, they converge to the same
state — no manual conflict resolution needed.

---

## 4. CRDTs (General Theory + Project Mapping)

All CRDTs must satisfy three algebraic merge properties (Shapiro et al., 2011):
- **Commutativity:** `merge(a, b) = merge(b, a)`
- **Associativity:** `merge(a, merge(b, c)) = merge(merge(a, b), c)`
- **Idempotence:** `merge(a, a) = a`

These three properties together form a join-semilattice, which guarantees convergence
regardless of message ordering or duplication. This is a mathematical guarantee, not a
heuristic.

**Our implementations** live in `src/replication/lattice.rs`:

### LWW Register (Last-Writer-Wins)

```rust
pub struct LwwRegister<T> {
    pub value: Option<T>,
    pub timestamp: LamportClock,
    pub tombstone: bool,
}
```

- `set(value, clock)` — update value with new timestamp
- `delete(clock)` — tombstone (value becomes None)
- `merge(other)` — higher `(timestamp.time, timestamp.replica_id)` wins
- Used for: Redis string values, hash field values

### GCounter (Grow-only Counter)

```rust
pub struct GCounter {
    counts: HashMap<ReplicaId, u64>,
}
```

- `increment(replica_id)` / `increment_by(replica_id, amount)` — only local replica increments
- `value()` — sum of all replica counts
- `merge(other)` — take max per replica
- Used for: monotonically increasing counters

### PNCounter (Positive-Negative Counter)

```rust
pub struct PNCounter {
    positive: GCounter,
    negative: GCounter,
}
```

- `increment(replica_id)` / `decrement(replica_id)` — separate positive/negative counters
- `value()` — `positive.value() - negative.value()` (can be negative)
- `merge(other)` — merge both internal counters
- Used for: Redis INCR/DECR on replicated keys

### GSet (Grow-only Set)

```rust
pub struct GSet<T: Clone + Eq + Hash> {
    elements: HashSet<T>,
}
```

- `add(element)` — add to set (never removed)
- `merge(other)` — set union
- Used for: tombstone tracking, seen-message sets

### ORSet (Observed-Remove Set)

```rust
pub struct ORSet<T: Clone + Eq + Hash> {
    elements: HashMap<T, HashSet<UniqueTag>>,
    next_sequence: HashMap<ReplicaId, u64>,
}
```

- `add(element, replica_id)` — add with unique tag
- `remove(element)` — remove all **currently observed** tags
- `merge(other)` — union of tags per element
- **Add-wins semantics:** concurrent add + remove = element is present
- Used for: Redis sets (SADD/SREM) under replication

---

## 5. Gossip Protocol

### Files

| File | Purpose |
|------|---------|
| `src/replication/gossip.rs` | Core gossip protocol logic |
| `src/replication/gossip_router.rs` | Selective routing via hash ring |
| `src/production/gossip_actor.rs` | Actor wrapping gossip protocol |
| `src/production/gossip_manager.rs` | Lifecycle management |

### How gossip works

1. Each replica accumulates **deltas** (changes since last gossip round)
2. Periodically, replica selects peers using hash ring (selective routing)
3. Delta is sent to selected peers
4. Peer merges delta using CRDT merge semantics
5. If Merkle tree digests diverge, full anti-entropy sync is triggered

### Anti-entropy (Merkle tree reconciliation)

**Stateright model:** `src/stateright/anti_entropy.rs` (`AntiEntropyModel`)
**TLA+ spec:** `specs/tla/AntiEntropy.tla`

Protocol:
1. Replicas exchange Merkle digests
2. If digests differ, mark peer as divergent
3. Initiate key-range sync for divergent subtrees
4. Merge using LWW semantics per key

---

## 6. Replication Data Flow

```
Client Write
    |
    v
ShardedActor (routes by key hash)
    |
    v
CommandExecutor (local execution)
    |
    v
ReplicatedShardActor (wraps delta)
    |
    v
GossipActor (selective routing)
    |
    v
Peer replicas (merge via CRDT)
```

**Files:**
- `src/production/sharded_actor.rs` — key routing
- `src/production/replicated_shard_actor.rs` — replication wrapping
- `src/production/gossip_actor.rs` — gossip dissemination

---

## 7. How to Add a New CRDT

1. Define the type in `src/replication/lattice.rs` with `merge()` method
2. Add Kani bounded proof in the `#[cfg(kani)] mod kani_proofs` block:
   - `verify_<type>_merge_commutative`
   - `verify_<type>_merge_idempotent`
3. Add DST harness in `src/replication/crdt_dst.rs`
4. Add Stateright model if architectural properties need exhaustive checking
5. Verify: `cargo test --lib crdt_dst -- --nocapture`

---

## 8. Verification Layers

| Layer | Tool | What it proves | Files |
|-------|------|---------------|-------|
| Specification | TLA+ | Protocol correctness under all interleavings | `specs/tla/*.tla` |
| Model checking | Stateright | Exhaustive state-space exploration of Rust models | `src/stateright/*.rs` |
| Bounded proofs | Kani | CRDT merge properties for all inputs within bounds | `src/replication/lattice.rs` |
| Simulation | DST | Correctness under fault injection with realistic workloads | `src/replication/crdt_dst.rs`, `tests/crdt_dst_test.rs` |
| Integration | Maelstrom | Distributed correctness under Jepsen-style partitions | `src/bin/maelstrom_kv_replicated.rs` |

---

## Anti-patterns

- **Assuming ordered delivery.** Gossip messages can arrive out of order. CRDTs handle this.
- **Using wall-clock time for ordering.** Use Lamport/vector clocks. Wall clocks can skew.
- **Blocking on network I/O in an actor.** Use async + timeout. Buggify simulates delays.
- **Manual conflict resolution.** Use CRDTs. If you find yourself writing `if conflict then pick_one`, you need a CRDT instead.
- **Testing replication without partitions.** Always test with `replication::SPLIT_BRAIN` and `replication::GOSSIP_DROP` enabled.
