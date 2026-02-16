# Claude Code Guidelines for redis-rust

## ⚠️ MANDATORY: Default Development Workflow

**For EVERY code change, follow this order:**

### 1. Plan First (Architecture)
Before writing any code:
- Identify which components are affected
- Check if new I/O abstractions are needed for testability
- Determine if actor boundaries need modification
- Document the approach in a brief plan

### 2. TigerStyle First
Every function/method must have:
- `debug_assert!` for preconditions at entry
- `debug_assert!` for postconditions/invariants after mutations
- Checked arithmetic (use `checked_add`, `checked_sub`, etc.)
- Explicit error handling (no `.unwrap()` in production paths)
- Early returns for error cases

### 3. DST-Compatible Design
Every new component must be:
- **Simulatable**: All I/O through trait abstractions
- **Deterministic**: No hidden randomness, use `SimulatedRng`
- **Time-controllable**: Use `VirtualTime`, not `std::time`

### 4. Write Tests Before/With Code
- Unit tests for pure logic
- DST tests with fault injection for I/O components
- Multiple seeds (minimum 10) for simulation tests
- Redis equivalence tests for command behavior changes
- Use Zipfian distribution (not uniform) for realistic workload tests

### 5. Design-by-Contract (TigerStyle Assertions)
- State invariants must be checkable at any point
- Add `verify_invariants()` methods to stateful structs
- Run invariant checks in debug builds after every mutation

Note: VOPR (Viewstamped Operation Replicator) is TigerBeetle's name for DST (Deterministic Simulation Testing) - see Phase 3 in the plan. The `verify_invariants()` pattern is a separate TigerStyle concept.

---

## Project Philosophy

This project follows **Simulation-First Development** inspired by FoundationDB and TigerBeetle. The core principle: **if you can't simulate it, you can't test it properly**.

## Architecture Principles

### 1. Deterministic Simulation Testing (DST)

All I/O operations must go through abstractions that can be:
- **Simulated**: Deterministic, controllable behavior
- **Fault-injected**: Network partitions, disk failures, message drops
- **Time-controlled**: Fast-forward time, test timeout scenarios

```rust
// GOOD: I/O through trait abstraction
trait ObjectStore: Send + Sync {
    async fn put(&self, key: &str, data: &[u8]) -> Result<()>;
    async fn get(&self, key: &str) -> Result<Vec<u8>>;
}

// BAD: Direct I/O that can't be simulated
std::fs::write(path, data)?;
```

### 2. Actor Architecture

Components communicate via message passing, not shared mutable state:

```rust
// GOOD: Actor owns state exclusively
struct PersistenceActor {
    state: PersistenceState,  // Owned, not shared
    rx: mpsc::Receiver<Message>,
}

// BAD: Shared state with mutex
struct SharedPersistence {
    state: Arc<Mutex<PersistenceState>>,  // Contention, hard to test
}
```

### 3. TigerStyle Coding

**Assertions (REQUIRED for every mutation):**
```rust
// GOOD: Assert preconditions and postconditions
fn hincrby(&mut self, field: &str, increment: i64) -> Result<i64> {
    debug_assert!(!field.is_empty(), "Precondition: field must not be empty");

    let new_value = self.value.checked_add(increment)
        .ok_or_else(|| Error::Overflow)?;

    self.value = new_value;

    debug_assert_eq!(self.get(field), Some(new_value),
        "Postcondition: value must equal computed result");

    Ok(new_value)
}

// BAD: No assertions, silent failures
fn hincrby(&mut self, field: &str, increment: i64) -> i64 {
    self.value += increment;  // Can overflow!
    self.value
}
```

**Checked Arithmetic (REQUIRED):**
- Use `checked_add`, `checked_sub`, `checked_mul`, `checked_div`
- Return explicit errors on overflow, never wrap silently
- Use `saturating_*` only when saturation is the correct behavior

**Control Flow:**
- Prefer early returns, avoid deep nesting
- No hidden allocations - be explicit about `Vec::with_capacity`
- No panics in production paths, explicit `Result<T, E>`

**Naming:**
- Functions that can fail: return `Result<T, E>`
- Functions that assert: prefix with `debug_assert!`
- Unsafe blocks: document why they're safe

### 4. Design-by-Contract (TigerStyle Assertions)

Every stateful struct should be verifiable:

```rust
// GOOD: Struct with verification method
impl RedisSortedSet {
    /// Verify all invariants hold - call in debug builds after mutations
    fn verify_invariants(&self) {
        debug_assert_eq!(
            self.members.len(),
            self.sorted_members.len(),
            "Invariant: members and sorted_members must have same length"
        );
        debug_assert!(
            self.is_sorted(),
            "Invariant: sorted_members must be sorted by (score, member)"
        );
        for (member, _) in &self.sorted_members {
            debug_assert!(
                self.members.contains_key(member),
                "Invariant: every sorted member must exist in members map"
            );
        }
    }

    pub fn add(&mut self, member: String, score: f64) {
        // ... mutation logic ...

        #[cfg(debug_assertions)]
        self.verify_invariants();
    }
}
```

**Design-by-Contract Checklist for New Structs:**
- [ ] Define all invariants in comments
- [ ] Implement `verify_invariants()` method
- [ ] Call verification after every public mutation method
- [ ] Add `#[cfg(debug_assertions)]` to avoid release overhead

### 5. Static Stability

Systems must remain stable under partial failures:
- Graceful degradation when dependencies fail
- Bounded queues with backpressure
- Timeouts on all external operations

## Testing Strategy

**Priority Order (most to least important):**

### 1. DST Tests (REQUIRED for I/O components)
```rust
#[test]
fn test_with_fault_injection() {
    for seed in 0..50 {  // Minimum 10 seeds, prefer 50+
        let mut harness = SimulationHarness::new(seed);
        harness.enable_chaos();  // Random faults

        // Run test scenario
        harness.run_scenario(|ctx| async {
            // Test logic here
        });

        harness.verify_invariants();
    }
}
```

**DST tests MUST:**
- Run with multiple seeds (minimum 10, ideally 50+)
- Include fault injection (network drops, delays, failures)
- Verify invariants after each operation
- Be deterministic given the same seed

### 2. Unit Tests (REQUIRED for pure logic)
- Test pure logic without I/O
- Use `InMemoryObjectStore` for storage tests
- Include edge cases: empty inputs, max values, overflow

### 3. Integration Tests
- Test component interactions
- Use real async runtime but simulated I/O

### 4. Redis Compatibility Tests (Official Tcl Suite)
Run the official Redis test suite against our implementation:
```bash
./scripts/run-redis-compat.sh                          # default test files
./scripts/run-redis-compat.sh tests/unit/type/string   # specific test file
```

### 5. Linearizability Tests (Jepsen-style)
- Use Maelstrom for distributed correctness
- Test under network partitions
- Verify consistency guarantees
- Run: `./maelstrom/maelstrom test -w lin-kv --bin ./target/release/maelstrom_kv_replicated`

### 6. Workload Realism (Zipfian Distribution)
Use Zipfian distribution for key access patterns in DST tests:
```rust
// GOOD: Realistic hot/cold key pattern
KeyDistribution::Zipfian { num_keys: 100, skew: 1.0 }

// BAD: Uniform distribution (unrealistic)
KeyDistribution::Uniform { num_keys: 100 }
```
With skew=1.0, top 10 keys receive ~40% of accesses (like real workloads).

## Key Files

| File | Purpose |
|------|---------|
| `src/io/mod.rs` | I/O abstractions (Clock, Network, RNG) |
| `src/simulator/` | DST harness and fault injection |
| `src/simulator/dst_integration.rs` | DST with Zipfian distribution |
| `src/streaming/simulated_store.rs` | Fault-injectable object store |
| `src/buggify/` | Probabilistic fault injection |
| `tests/redis-tests/` | Official Redis Tcl test suite (git submodule) |
| `scripts/run-redis-compat.sh` | Wrapper to run Tcl tests against our server |

## Common Patterns

### Creating Testable Components

```rust
// 1. Define trait for the I/O operation
pub trait ObjectStore: Send + Sync + 'static {
    fn put(&self, key: &str, data: &[u8]) -> impl Future<Output = IoResult<()>>;
}

// 2. Create production implementation
pub struct LocalFsObjectStore { path: PathBuf }

// 3. Create simulated implementation with fault injection
pub struct SimulatedObjectStore {
    inner: InMemoryObjectStore,
    fault_config: FaultConfig,
    rng: SimulatedRng,
}

// 4. Use generic in component
pub struct StreamingPersistence<S: ObjectStore> {
    store: S,
    // ...
}
```

### Actor Shutdown Pattern

```rust
enum Message {
    DoWork(Work),
    Shutdown { response: oneshot::Sender<()> },
}

async fn run(mut self) {
    while let Some(msg) = self.rx.recv().await {
        match msg {
            Message::DoWork(w) => self.handle_work(w).await,
            Message::Shutdown { response } => {
                self.cleanup().await;
                let _ = response.send(());
                break;
            }
        }
    }
}
```

### Bridging Sync to Async

When you need to connect sync code (like command execution) to async actors:

```rust
// Use std::sync::mpsc for fire-and-forget from sync context
let (tx, rx) = std::sync::mpsc::channel();

// Bridge task drains sync channel into async actor
async fn bridge(rx: Receiver<Delta>, actor: ActorHandle) {
    loop {
        // Use recv_timeout to stay responsive to shutdown
        if let Some(delta) = rx.recv_timeout(Duration::from_millis(50)) {
            actor.send(delta);
        }
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
    }
}
```

## Debugging Tips

1. **Seed-based reproduction**: All simulations use seeds. Save failing seeds to reproduce bugs.
2. **Trace logging**: Use `tracing` crate with structured logging.
3. **Invariant checks**: Add assertions for state invariants after each operation.

## Performance Guidelines

1. **Batch operations**: Accumulate deltas in write buffer before flushing
2. **Async I/O**: Use `tokio::spawn` for background operations
3. **Zero-copy where possible**: Use `Bytes` for large data transfers
4. **Profile before optimizing**: Use `cargo flamegraph`

## Benchmarking Requirements

**All performance comparisons MUST use Docker containers** to ensure fair, reproducible results.

### Why Docker-Only Benchmarks?

| Concern | Docker Solution |
|---------|-----------------|
| Resource fairness | Both servers get identical CPU/memory limits |
| Environment consistency | Same OS, kernel, networking stack |
| Reproducibility | Anyone can run identical benchmarks |
| Apples-to-apples | Eliminates "my machine is faster" bias |

### Docker Benchmark Configuration

Location: `docker-benchmark/`

| Setting | Value | Rationale |
|---------|-------|-----------|
| CPU Limit | 2 cores | Realistic server constraint |
| Memory Limit | 1GB | Realistic server constraint |
| Network | Host networking | Eliminates Docker NAT overhead |
| Requests | 100,000 | Statistically significant |
| Clients | 50 concurrent | Realistic load |
| Pipeline depths | 1, 16, 64 | Tests different access patterns |

### Running Docker Benchmarks

```bash
# REQUIRED for any performance claims
cd docker-benchmark
./run-benchmarks.sh

# This runs:
# 1. Official Redis 7.4 (port 6379)
# 2. Rust implementation (port 3000)
# 3. Both non-pipelined and pipelined tests
```

### What NOT to Use for Comparisons

```bash
# DON'T use these for Redis vs Rust comparisons:
cargo run --release --bin quick_benchmark  # Local only, no Redis comparison
cargo run --release --bin benchmark        # Local only, unfair conditions
```

The local benchmarks (`quick_benchmark`, `benchmark`) are useful for:
- Quick smoke tests during development
- Profiling and optimization work
- Regression detection between commits

But they MUST NOT be used for Redis vs Rust performance claims in documentation.

### Updating BENCHMARK_RESULTS.md

When updating performance numbers:

1. **Always run Docker benchmarks** - never use local numbers for comparisons
2. **Run multiple times** - take median of 3 runs to reduce variance
3. **Document the environment** - include Docker version, host OS
4. **Include all pipeline depths** - P=1, P=16, P=64 tell different stories

```bash
# Example workflow for updating benchmarks
cd docker-benchmark
for i in 1 2 3; do
    echo "=== Run $i ==="
    ./run-benchmarks.sh 2>&1 | tee run_$i.log
done
# Use median results in BENCHMARK_RESULTS.md
```

## Commit Requirements

**Every commit must be fully tested and documented:**

### Pre-Commit Checklist

1. **Full Test Suite**: Run `cargo test --release` - all tests must pass
2. **DST Tests**: Run streaming and simulator DST tests with multiple seeds
3. **Local Smoke Test**: Run `cargo run --release --bin quick_benchmark` for quick validation
4. **Docker Benchmarks**: Run `docker-benchmark/run-benchmarks.sh` if updating performance claims
5. **Documentation**: Update `BENCHMARK_RESULTS.md` with Docker benchmark results if metrics change

### Commit Workflow

```bash
# 1. Run full test suite
cargo test --release

# 2. Run DST batch tests (if modifying streaming/persistence)
cargo test streaming_dst --release

# 3. Run Maelstrom linearizability tests (if modifying replication)
./maelstrom/maelstrom test -w lin-kv --bin ./target/release/maelstrom_kv_replicated \
    --node-count 3 --time-limit 60 --rate 100

# 4. Quick local smoke test
cargo run --release --bin quick_benchmark

# 5. Docker benchmarks (REQUIRED for performance claims)
cd docker-benchmark && ./run-benchmarks.sh

# 6. Update BENCHMARK_RESULTS.md with Docker benchmark results

# 7. Commit with descriptive message
git commit -m "Description of changes

- What was added/changed
- Test results summary
- Docker benchmark comparison (if performance-related)"
```

### What Must Be Updated

| Change Type | Required Updates |
|-------------|-----------------|
| New feature | Tests, README feature list |
| Performance change | Docker benchmarks, BENCHMARK_RESULTS.md (Docker numbers only) |
| Bug fix | Regression test |
| Replication change | Maelstrom test run |
| Streaming change | DST tests with fault injection |
| Command behavior change | Redis equivalence test |
| Any Redis comparison | Docker benchmarks (NEVER local benchmarks) |

### Benchmark Documentation Format

When updating `BENCHMARK_RESULTS.md`, include:
```markdown
## [Date] - [Change Description]

### Test Configuration
- **Method**: Docker benchmarks (docker-benchmark/run-benchmarks.sh)
- **Docker**: [version]
- **Host OS**: [OS version]
- **CPU Limit**: 2 cores per container
- **Memory Limit**: 1GB per container

### Results (Docker - Fair Comparison)
| Operation | Redis 7.4 | Rust Implementation | Relative |
|-----------|-----------|---------------------|----------|
| SET (P=1) | X req/s   | Y req/s             | Z%       |
| GET (P=1) | X req/s   | Y req/s             | Z%       |
| SET (P=16)| X req/s   | Y req/s             | Z%       |
| GET (P=16)| X req/s   | Y req/s             | Z%       |
```

**Important**: All Redis vs Rust comparison numbers MUST come from Docker benchmarks.

## Architecture Decision Records (ADRs)

ADRs in `docs/adr/` are **living decision logs** that track architectural decisions as they evolve. They are not static documents - update them as you work.

### ADR Index

| ADR | Title | Summary |
|-----|-------|---------|
| [001](docs/adr/001-simulation-first-development.md) | Simulation-First Development | FoundationDB/TigerBeetle-style DST |
| [002](docs/adr/002-actor-per-shard-architecture.md) | Actor-per-Shard Architecture | Lock-free message passing |
| [003](docs/adr/003-tigerstyle-coding-standards.md) | TigerStyle Coding Standards | Safety-first engineering |
| [004](docs/adr/004-anna-kvs-crdt-replication.md) | Anna KVS CRDT Replication | Eventual/causal consistency |
| [005](docs/adr/005-streaming-persistence.md) | Streaming Persistence | Cloud-native S3 storage |
| [006](docs/adr/006-zero-copy-resp-parser.md) | Zero-Copy RESP Parser | High-performance parsing |
| [007](docs/adr/007-feature-flag-optimizations.md) | Feature Flag Optimizations | Measurable performance tuning |
| [008](docs/adr/008-docker-benchmark-methodology.md) | Docker Benchmark Methodology | Fair performance comparisons |
| [009](docs/adr/009-security-tls-acl.md) | Security - TLS and ACL | Optional TLS and Redis 6.0+ ACL |

### When to Update an ADR

**During development**, add a Decision Log entry when you:
- Choose between alternative implementations
- Discover constraints that affect the architecture
- Defer or reject a planned feature
- Change approach based on learnings
- Make trade-offs during implementation

**After commits**, update Implementation Status when:
- New components are implemented
- Features are validated/tested
- Items move from "Not Yet Implemented" to "Implemented"

### How to Update an ADR

1. **Add a Decision Log entry** (most common):
   ```markdown
   | 2026-01-15 | Use batch inserts for persistence | Reduces write amplification |
   ```

2. **Update Implementation Status** tables as work progresses

3. **Update main sections** only if the decision fundamentally changes the approach

### Creating a New ADR

When making a decision that doesn't fit existing ADRs:
1. Copy `docs/adr/000-template.md` to `docs/adr/NNN-descriptive-title.md`
2. Fill in Context, Decision, Consequences
3. Add initial Decision Log entry
4. Update `docs/adr/README.md` index

## Dependencies

Core dependencies and their purposes:
- `tokio`: Async runtime
- `bincode`: Binary serialization (3-5x smaller than JSON)
- `crc32fast`: Checksums for data integrity
- `tracing`: Structured logging
