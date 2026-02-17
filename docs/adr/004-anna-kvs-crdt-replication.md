# ADR-004: Anna KVS CRDT Replication

## Status

Accepted

## Context

Redis provides replication through a single-leader model where writes go to the primary and replicate to secondaries. This provides strong consistency but:

1. **Leader bottleneck**: All writes funnel through one node
2. **Failover complexity**: Requires consensus for leader election
3. **Write availability**: Writes blocked during leader failure
4. **Cross-region latency**: Synchronous replication adds RTT

For a cache system where **availability often matters more than strict consistency**, we can choose a different trade-off. Anna KVS demonstrated that CRDTs (Conflict-free Replicated Data Types) enable:

- **Coordination-free writes**: Any node can accept writes
- **Automatic conflict resolution**: CRDTs merge deterministically
- **High availability**: No single point of failure
- **Horizontal scalability**: Add nodes without coordination

## Decision

We will implement **Anna KVS-style CRDT replication** with:

### 1. Configurable Consistency Levels

```rust
pub enum ConsistencyLevel {
    /// Last-Writer-Wins with Lamport clocks
    Eventual,
    /// Vector clocks for causal ordering
    Causal,
}
```

### 2. CRDT Types

| CRDT | Use Case | Merge Strategy |
|------|----------|----------------|
| LWW Register | String values | Highest Lamport timestamp wins |
| G-Counter | Increment-only counters | Max per-replica count |
| PN-Counter | Inc/Dec counters | Separate G-Counters for +/- |
| OR-Set | Sets with add/remove | Unique tags for causality |
| G-Set | Add-only sets | Union |

### 3. Gossip Protocol

```
Node 1                    Node 2                    Node 3
  |                         |                         |
[LWW Register]  <--Gossip-->  [LWW Register]  <--Gossip-->  [LWW Register]
  |                         |                         |
[Lamport Clock]           [Lamport Clock]           [Lamport Clock]
```

- **Push-based gossip**: Nodes periodically push updates to peers
- **Anti-entropy**: Merkle tree-based consistency verification
- **Hot key detection**: Automatic increased replication for high-traffic keys

### 4. Consistency Guarantees

| Mode | Single-Node | Multi-Node |
|------|-------------|------------|
| Linearizable | Yes (verified via Maelstrom) | No |
| Eventual | Yes | Yes (CRDT convergence verified) |
| Causal | Yes | Yes (vector clock verified) |

**Important**: Multi-node mode provides **eventual consistency**, not linearizability. This is by design for coordination-free scalability.

## Consequences

### Positive

- **High availability**: No leader election, writes always accepted
- **Partition tolerance**: Nodes operate independently during partitions
- **Scalability**: Add nodes without coordination overhead
- **Low latency**: No synchronous replication delay
- **Automatic recovery**: CRDTs converge after partition heals

### Negative

- **Eventual consistency**: Clients may read stale data
- **CRDT limitations**: Not all Redis operations map to CRDTs
- **Memory overhead**: Vector clocks and unique tags add storage
- **Complexity**: Understanding CRDT semantics requires training

### Risks

- **Conflict semantics**: LWW may surprise users expecting "last actual write"
- **Counter accuracy**: PN-Counters may diverge temporarily
- **Anti-entropy cost**: Merkle tree computation adds overhead
- **No transactions**: MULTI/EXEC incompatible with CRDTs

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-03 | Initial ADR created | Anna KVS model chosen for coordination-free scalability |
| 2026-01-03 | Use Lamport clocks for LWW | Simple, sufficient for eventual consistency |
| 2026-01-04 | Add Vector clocks for causal mode | Required for happens-before tracking |
| 2026-01-05 | Implement OR-Set for Redis Sets | Handles concurrent add/remove correctly |
| 2026-01-05 | Add anti-entropy with Merkle trees | Detect and repair divergence efficiently |
| 2026-01-06 | Implement hot key detection | Zipfian workloads need adaptive replication |
| 2026-01-06 | Verify convergence with DST | Multi-seed tests confirm CRDT correctness |

## Implementation Status

### Implemented

| Component | Location | Status |
|-----------|----------|--------|
| LamportClock | `src/replication/lattice.rs` | Monotonic logical clock |
| VectorClock | `src/replication/lattice.rs` | Causal ordering |
| LwwRegister | `src/replication/lattice.rs` | Last-Writer-Wins register |
| GCounter | `src/replication/lattice.rs` | Grow-only counter |
| PNCounter | `src/replication/lattice.rs` | Increment/decrement counter |
| ORSet | `src/replication/lattice.rs` | Observed-Remove set |
| GSet | `src/replication/lattice.rs` | Grow-only set |
| GossipRouter | `src/replication/gossip_router.rs` | Message routing |
| HashRing | `src/replication/hash_ring.rs` | Consistent hashing |
| AntiEntropyManager | `src/replication/anti_entropy.rs` | Merkle tree sync |
| ReplicatedState | `src/replication/state/` | CRDT-backed state (module: shard_state.rs, crdt_value.rs, delta.rs, replicated_value.rs) |
| CRDT DST Tests | `src/replication/crdt_dst.rs` | Convergence verification |

### Validated

- CRDT convergence verified with 16 DST tests (100+ seeds each = 1,600+ deterministic runs)
- Multi-node replication verified with 27+ tests (eventual consistency: 9, causal: 10, anti-entropy: 8)
- Partition tolerance verified via DST harnesses in `src/simulator/partition_tests.rs`
- Maelstrom single-node linearizability passes

### Not Yet Implemented

| Component | Notes |
|-----------|-------|
| CRDT-aware MGET/MSET | MGET/MSET exist in executor but don't generate replication deltas |
| Read-your-writes consistency | Session guarantees not implemented |

### Previously Listed as Not Implemented (Now Done)

| Component | Location | When |
|-----------|----------|------|
| Delta-state CRDTs | `src/replication/state/delta.rs`, `src/replication/gossip.rs` | Delta-based gossip with `GossipMessage::DeltaBatch`, `drain_deltas()`/`apply_delta()`, selective routing via hash ring |

## References

- [Anna KVS Paper](https://dsf.berkeley.edu/jmh/papers/anna_ieee18.pdf)
- [CRDTs: Consistency without consensus](https://arxiv.org/abs/1805.06358)
- [A comprehensive study of CRDTs](https://hal.inria.fr/inria-00555588/document)
- [Riak CRDT documentation](https://docs.riak.com/riak/kv/latest/developing/data-types/)
