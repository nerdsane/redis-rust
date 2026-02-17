# ADR-001: Simulation-First Development (DST)

## Status

Accepted

## Context

Building reliable distributed systems is notoriously difficult. Traditional testing approaches fail to catch subtle bugs that only manifest under specific timing conditions, network partitions, or failure scenarios. These "Heisenbugs" can cause data loss, inconsistency, or availability issues in production.

Industry leaders like FoundationDB and TigerBeetle have demonstrated that **Deterministic Simulation Testing (DST)** can achieve exceptional reliability by:

1. Making all non-determinism (time, network, disk I/O, randomness) controllable
2. Running thousands of test iterations with different random seeds
3. Injecting faults probabilistically to explore failure modes
4. Reproducing any failure with the exact same seed

For a Redis-compatible cache that aims to support replication and persistence, we need the same level of confidence in correctness that these systems achieve.

## Decision

We will follow **Simulation-First Development** where:

1. **All I/O operations go through trait abstractions** that can be swapped between production and simulated implementations
2. **All randomness uses seeded RNGs** (`SimulatedRng` with ChaCha8) for reproducibility
3. **All time operations use controllable clocks** (`VirtualTime`) that can be advanced deterministically
4. **Fault injection is built into simulated implementations** with configurable probabilities
5. **Tests run with multiple seeds** (minimum 10, ideally 50+) to explore the state space

### Core Abstractions

```rust
// I/O through traits
pub trait ObjectStore: Send + Sync {
    async fn put(&self, key: &str, data: &[u8]) -> Result<()>;
    async fn get(&self, key: &str) -> Result<Vec<u8>>;
}

// Simulated implementation with fault injection
pub struct SimulatedObjectStore {
    inner: InMemoryObjectStore,
    fault_config: FaultConfig,
    rng: SimulatedRng,
}

// Deterministic time
pub struct VirtualTime {
    current: AtomicU64,
}

// Seeded randomness
pub struct SimulatedRng {
    inner: ChaCha8Rng,
}
```

### Testing Strategy Priority

1. **DST Tests** (REQUIRED for I/O components) - Multi-seed chaos testing
2. **Unit Tests** (REQUIRED for pure logic) - No I/O, fast execution
3. **Integration Tests** - Real async runtime, simulated I/O
4. **Redis Equivalence Tests** - Differential testing against real Redis
5. **Linearizability Tests** - Maelstrom/Jepsen for consistency verification

## Consequences

### Positive

- **Bug reproduction**: Any failure can be reproduced with the exact seed
- **Comprehensive coverage**: Thousands of scenarios tested automatically
- **Confidence in correctness**: Faults are tested, not assumed away
- **Fast iteration**: Simulated tests run in milliseconds, not minutes
- **Documentation**: Tests serve as executable specifications

### Negative

- **Initial complexity**: Must design for simulation from the start
- **Abstraction overhead**: All I/O must go through traits
- **Learning curve**: Team must understand DST methodology
- **Refactoring cost**: Retrofitting simulation to existing code is expensive

### Risks

- **Simulation fidelity**: Simulated behavior may diverge from production
- **Seed coverage**: Random seeds may not explore all edge cases
- **Performance overhead**: Trait abstractions may add runtime cost in production

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-03 | Initial ADR created | DST approach inspired by FoundationDB/TigerBeetle for reliability |
| 2026-01-03 | Use ChaCha8 for SimulatedRng | Cryptographic quality randomness, fast, reproducible |
| 2026-01-04 | Add fault injection to SimulatedObjectStore | Test persistence under write failures, corruption |
| 2026-01-04 | Require minimum 10 seeds per DST test | Balance between coverage and test runtime |
| 2026-01-05 | Add Zipfian distribution for workload realism | Uniform distribution unrealistic; hot keys dominate real workloads |
| 2026-01-06 | Integrate Maelstrom for linearizability testing | External validation of consistency guarantees |

## Implementation Status

### Implemented

| Component | Location | Status |
|-----------|----------|--------|
| SimulatedRng | `src/io/simulation.rs` | ChaCha8-based seeded RNG |
| VirtualTime | `src/simulator/time.rs` | Controllable time abstraction |
| SimulationHarness | `src/simulator/harness.rs` | Test harness with fault injection |
| SimulatedObjectStore | `src/streaming/simulated_store.rs` | Fault-injectable object store |
| DST Integration | `src/simulator/dst_integration.rs` | Zipfian workloads, chaos testing |
| Multi-node DST | `src/simulator/multi_node.rs` | Distributed system simulation |
| Partition Tests | `src/simulator/partition_tests.rs` | Network partition scenarios |
| Crash Tests | `src/simulator/crash.rs` | Node crash and recovery |

### Validated

- Multi-seed DST tests run with 50+ seeds
- Fault injection catches persistence bugs
- Zipfian distribution matches production access patterns
- Maelstrom single-node linearizability passes

### Not Yet Implemented

| Component | Notes |
|-----------|-------|
| Low-level disk fault injection | Disk fault constants defined in buggify but not wired to actual I/O (object store faults ARE wired) |

### Previously Listed as Not Implemented (Now Done)

| Component | Location | When |
|-----------|----------|------|
| Network fault injection | `src/io/simulation.rs` | All 8 network faults wired: PACKET_DROP, REORDER, DUPLICATE, DELAY, CORRUPT, CONNECTION_RESET, CONNECT_TIMEOUT, PARTIAL_WRITE |
| Clock skew simulation | `src/io/simulation.rs` | `ClockOffset` with per-node drift_ppm, fixed offsets, tested |

## References

- [FoundationDB Testing](https://apple.github.io/foundationdb/testing.html)
- [TigerBeetle Simulation](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/SIMULATION.md)
- [Will Wilson: Testing Distributed Systems](https://www.youtube.com/watch?v=4fFDFbi3toc)
- [Jepsen](https://jepsen.io/)
- [Maelstrom](https://github.com/jepsen-io/maelstrom)
