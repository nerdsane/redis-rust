# redis-rust: A Case Study in AI-Assisted Systems Programming with Deterministic Verification

**Authors:** Sesh Nalla and Claude Code (Anthropic, Opus 4.5 → Opus 4.6)

**Abstract:** We describe redis-rust, an experimental Redis-compatible in-memory data store written in Rust and co-authored by a human systems engineer and Claude Code (Anthropic, Opus 4.5 → Opus 4.6). The project began as a deliberate test: could human-Claude collaboration produce a distributed systems implementation that is not merely plausible but verifiably correct? The system implements Redis commands across strings, lists, sets, hashes, sorted sets, bitmaps, transactions, Lua scripting, and key expiration, with CRDT-based multi-node replication using gossip and anti-entropy protocols. A hybrid persistence layer combines a local Write-Ahead Log (WAL) with group commit for zero-RPO durability and cloud-native streaming to object storage (S3/local) for long-term backup. Correctness is established through a multi-layer verification pyramid: deterministic simulation testing with fault injection, the official Redis Tcl test suite, Maelstrom/Jepsen linearizability checking, Stateright exhaustive model checking, TLA+ specifications, and Docker-based performance benchmarking. Each verification method was chosen because it has objective, mechanical pass/fail criteria that do not depend on who wrote the code or the tests. On equivalent hardware, throughput reaches 77--99% of Redis 7.4. This is not a production Redis replacement. It is an honest case study in what AI-assisted systems programming can and cannot do today, with a reusable verification methodology as its primary artifact.

---

## 1. Introduction

### 1.1 Genesis

In late 2025, a step-change in AI code generation capability made a project like this feasible. Models had been generating code for years -- from simple completions to multi-file scaffolding -- but the quality and coherence improved rapidly across 2024-2025. By late 2025, with Anthropic's Claude Opus 4.5 (the first model to exceed 80% on SWE-bench Verified, scoring 80.9%), Claude Code could sustain coherent implementations across thousands of lines, maintain internal consistency across module boundaries, hold architectural context across an entire codebase, and reason about subtle correctness properties like memory safety, concurrent access, and distributed protocol invariants. Claude could read a TLA+ spec and produce a Rust implementation that preserved the spec's invariants. It could be asked to "add WATCH support to MULTI/EXEC across a sharded architecture" and produce a design that correctly identified the value-snapshot approach as the only option that avoids shared mutable state -- then implement it.

We wanted to test this in the hardest way we could think of that was still tractable for two people (one human, one AI). Distributed systems are notoriously difficult. They require reasoning about concurrency, partial failure, network nondeterminism, and subtle invariants that hold across multiple machines. They are also notoriously difficult to *test* -- the interesting bugs only appear under specific timing conditions that unit tests rarely exercise. If AI-assisted development could produce a verifiably correct distributed system, that would be a meaningful data point. If it could not, understanding *where* it failed would be equally valuable.

The question was not "can an AI write code that compiles?" That bar was cleared years ago. The question was: **can a human-AI team produce systems code that survives the same verification methods we would apply to human-written code?** Not code that looks right. Code that *is* right, as far as we can tell, under adversarial testing.

### 1.2 Why Redis

Redis was chosen as the implementation target for several pragmatic reasons:

**Familiar semantics.** Redis's command set is well-documented, widely understood, and has sharp behavioral edges (WRONGTYPE errors, empty-collection auto-deletion, expiration-on-access) that make a faithful implementation non-trivial. It is easy to explain what the system should do. It is hard to get every edge case right.

**An official test suite exists.** The Redis project maintains a Tcl-based test suite that exercises command behavior at a level of detail that no handwritten test suite could match in a reasonable timeframe. Passing these tests provides a confidence signal that is independent of the authors -- neither the human nor Claude wrote the tests, and neither can unconsciously bias them toward the implementation.

**Performance is measurable.** `redis-benchmark` is a standard tool. Running the same benchmark against Redis 7.4 and our implementation on identical hardware gives a concrete, reproducible performance comparison. No synthetic microbenchmarks, no favorable conditions -- just the standard tool with default settings.

**The scope is bounded.** A full Redis reimplementation is enormous, but a useful subset (strings, lists, sets, hashes, sorted sets, transactions, expiration, Lua scripting) is achievable in weeks rather than years. We could reach a meaningful "checkpoint" -- passing official tests, matching performance -- without committing to a multi-year project.

### 1.3 Why This Paper

This paper is not a product announcement. redis-rust is explicitly not production software, and we say so in the first line of its documentation.

What we think is worth sharing is the *process*: how we structured verification to keep Claude honest, what kinds of bugs Claude introduced and how they were caught, where human judgment was irreplaceable, and where the acceleration was genuine. The verification pyramid is a methodology that could be applied to any AI-assisted systems project.

We also want to be straightforward about the limitations. The multi-node replication is eventually consistent by design, not linearizable. WAL persistence covers string and hash types but not yet lists, sets, or sorted sets. The Tcl test suite crashes on unimplemented commands rather than gracefully skipping them, so our "pass rate" reflects command coverage as much as correctness.

### 1.4 Contributions

This paper makes three contributions:

1. **A verification methodology for AI-assisted systems code.** The verification pyramid provides defense-in-depth against the specific failure modes of model-generated code: plausible-looking implementations that are subtly wrong, correct logic with incorrect error messages, and working code that silently regresses under load. The verification tools themselves are also Claude-generated -- but each layer was chosen because it has **objective, mechanical pass/fail criteria** independent of the author. TLC exhaustively enumerates states. The Tcl suite was written by the Redis authors. Maelstrom/Knossos applies a formal linearizability checker. These tools cannot be "fooled" by plausible-but-wrong specifications.

2. **An architecture case study.** The actor-per-shard design, CRDT-based replication with gossip protocol, and Lua scripting integration demonstrate how a human-Claude team navigated real systems design decisions -- lock-free concurrency via message passing, connection-level vs. shard-level transaction state, virtual time for deterministic simulation. We document the trade-offs explicitly, including the ones we got wrong on the first attempt.

3. **An honest accounting of results.** Hundreds of passing tests across multiple suites, fully passing Tcl compatibility for implemented commands, 5-node Maelstrom validation, and 77--99% of Redis 7.4 throughput. Also: several Tcl suites not attempted, no pub/sub, no linearizable replication. Six production-grade bugs caught by DST that would have shipped in a less-tested implementation (detailed in Section 5.1). We report what works, what does not, and what we learned from each failure.

---

## 2. Architecture and Trade-offs

### 2.1 Actor-per-Shard Design

The server partitions keyspace across N shards, where each shard is a tokio task that exclusively owns a `CommandExecutor`. The number of shards is configurable at startup via `perf_config.toml` (default: 16). Shard actors communicate through unbounded mpsc channels and exclusively own their data -- there are no locks within the per-shard data storage. The connection handler acquires a `parking_lot::RwLock` for ACL permission checks, bypassed on the GET/SET fast path.

```
                        ┌───────────────────────────────────┐
                        │        Connection Handler         │
                        │  ┌─────────┐  ┌───────────────┐  │
  Client ──TCP──▶       │  │  RESP   │  │  MULTI/EXEC   │  │
                        │  │ Parser  │  │ State Machine  │  │
                        │  └────┬────┘  └───────────────┘  │
                        └───────┼───────────────────────────┘
                                │
                          hash(key) % N
                       ┌────────┼────────┐
                       ▼        ▼        ▼
                 ┌──────────┐ ┌──────────┐ ┌──────────┐
                 │ Shard 0  │ │ Shard 1  │ │ Shard N  │
                 │ (Actor)  │ │ (Actor)  │ │ (Actor)  │
                 │          │ │          │ │          │
                 │ Executor │ │ Executor │ │ Executor │
                 │ HashMap  │ │ HashMap  │ │ HashMap  │
                 └──────────┘ └──────────┘ └──────────┘
                      ▲            ▲            ▲
                      └──── mpsc channels ──────┘
                        (no locks on shard data)
```

Multi-key commands (MGET, MSET, DEL, EXISTS) are decomposed at the `ShardedActorState` layer: keys are grouped by target shard, dispatched concurrently via `join_all`, and results are reassembled in the original key order. Commands that require global visibility -- DBSIZE, SCAN, KEYS, FLUSHDB -- fan out to all shards and aggregate.

This design has three consequences worth noting. First, each shard's `HashMap` fits in a CPU cache hierarchy partition, reducing cross-core cache invalidation. Second, because the `CommandExecutor` is single-threaded within its task, all operations on a given key are linearizable without coordination. Third, the architecture admits a fast path for GET and SET that bypasses `Command` enum construction entirely: the connection handler can parse RESP bytes directly into a `FastGet` or `FastSet` message, cutting per-operation allocation by one enum variant and one string copy.

### 2.2 Connection-Level Transactions

Redis transactions (MULTI/EXEC) present a design challenge in a sharded system. A MULTI block may queue commands targeting different shards, but EXEC must execute them atomically relative to the client's view.

Our solution places all transaction state in the connection handler, not in the per-shard executor. When a client issues MULTI, the handler enters queuing mode: subsequent commands accumulate in `transaction_queue: Vec<Command>` rather than being dispatched. On EXEC, the handler drains the queue and dispatches each command to its target shard sequentially, collecting responses into an array. During a MULTI block, the fast path is explicitly disabled (`!self.in_transaction` guards the branch) to prevent atomicity violations.

```
                    MULTI              queue cmds            EXEC
  ┌──────┐       ┌────────┐         ┌────────────┐       ┌──────────┐
  │ idle │──────▶│queuing │────────▶│  queuing   │──────▶│ execute  │──▶ idle
  └──────┘       └────────┘   SET   │ [SET,INCR] │       │  queue   │
     ▲                        INCR  └────────────┘       │ check    │
     │                                    │              │ watches  │
     │                               DISCARD             └──────────┘
     │                                    │                   │
     │                                    ▼              watch fail?
     │                                 ┌──────┐               │
     └─────────────────────────────────│ idle │◀──────────────┘
                                       └──────┘          return nil
```

WATCH implements optimistic locking through value-snapshot comparison. When the client issues `WATCH key`, the handler fetches the current value and stores `(key, RespValue)` pairs. At EXEC time, each watched key is re-fetched and compared using structural equality. If any value differs, EXEC returns a null array (transaction aborted).

```
  Connection A                  Connection B                Server State
  ──────────────────────────────────────────────────────────────────────
  WATCH x
  ◀── snapshot: x = "foo" ─────────────────────────────── x = "foo"
                                SET x "bar"
                                ◀── OK ────────────────── x = "bar"
  MULTI
  SET x "baz"  → queued
  EXEC
  ◀── GET x → "bar"
       "bar" ≠ "foo" (snapshot)
  ◀── nil (aborted) ──────────────────────────────────── x = "bar"
                                                         (unchanged)
```

This is an intentional divergence from Redis, which tracks per-key dirty flags. Redis aborts a watched transaction if *any* write touches the key, even if the write is a no-op that leaves the value unchanged. Our value-based comparison accepts such no-op writes. The reason is architectural: dirty flags require per-key mutation tracking, which is infeasible across shards without shared state -- exactly what the actor model avoids.

### 2.3 CRDT Replication

The replication layer draws from Anna KVS [Wu et al., IEEE ICDE 2018]. Each key-value pair is wrapped in a `ReplicatedValue` containing a last-writer-wins register timestamped with a Lamport clock. The CRDT library also includes `GCounter`, `PNCounter`, `ORSet`, `GSet`, and `VectorClock` types, with merge commutativity and idempotence verified through Kani bounded proofs (LWW, GCounter, PNCounter, VectorClock) and exhaustive unit tests.

```
  Node 1                      Node 2                      Node 3
  ┌──────────────┐            ┌──────────────┐            ┌──────────────┐
  │ LWW Register │◀──gossip──▶│ LWW Register │◀──gossip──▶│ LWW Register │
  │ Lamport Clock│            │ Lamport Clock│            │ Lamport Clock│
  │ Vector Clock │            │ Vector Clock │            │ Vector Clock │
  └──────┬───────┘            └──────┬───────┘            └──────┬───────┘
         │                           │                           │
         └───────── Merkle Tree Anti-Entropy ────────────────────┘
                   (partition healing, O(log n) sync)
```

Replication is delta-based: on each write, the server produces a `ReplicationDelta` containing the key, the new value, and the source replica ID. In broadcast mode, all deltas go to all peers. In selective mode, a `GossipRouter` consults a consistent hash ring (150 virtual nodes per physical node, replication factor 3) to route each delta only to responsible nodes -- reducing gossip traffic from O(n) to O(RF) per delta.

Anti-entropy runs as a background protocol using Merkle tree digests. Each node maintains a 256-bucket Merkle tree over its keyspace. Periodically, nodes exchange `StateDigest` messages. Divergent buckets are identified in O(log n) comparisons, and only the keys in those buckets are synced.

### 2.4 Scaling Behavior

A scaling test sweeps 1, 2, 4, 8, 16, and 32 shards on 2-CPU Docker containers:

```
  SET P=16 throughput (K rps) vs shard count (2 CPUs)

  1100 ┤
  1000 ┤    ██
   900 ┤ ██ ██ ██
   800 ┤ ██ ██ ██ ██
   700 ┤ ██ ██ ██ ██         ██
   600 ┤ ██ ██ ██ ██ ██      ██
   500 ┤ ██ ██ ██ ██ ██      ██
       └──────────────────────────
         1  2  4  8  16     32
                shards
```

Key finding: with 2 available cores, 2-4 shards peak at roughly 1M SET/s pipelined. Beyond 4 shards, throughput degrades as context-switching overhead dominates. The optimal shard count is bounded by available CPU cores, not by any inherent limit in the architecture.

### 2.5 Explicit Trade-offs

| Aspect | Redis 7.4 | redis-rust | Rationale |
|--------|-----------|------------|-----------|
| WATCH semantics | Dirty-flag per key | Value-snapshot comparison | Cannot track mutations across shards |
| Persistence | RDB + AOF | WAL (group commit) + S3 streaming | Local fsync for zero-RPO, async cloud backup |
| Blocking ops | BLPOP, BRPOP | Not implemented | Requires cross-shard signaling |
| Bitmaps | Full support | SETBIT/GETBIT | Operates on string (SDS) values |
| Streams | Full support | Not implemented | Low priority for verification research |
| Thread model | Single-threaded + I/O | Actor-per-shard (tokio) | Multi-core scaling without a GIL |
| Transaction scope | All keys visible | Queued at connection, per-shard dispatch | Atomicity from client perspective |
| Cluster mode | Hash slots | Single-node sharding + CRDT gossip | Hash slots require client-side routing |
| Lua key access | All keys | Only executing shard (or single-shard mode) | Multi-shard Lua needs distributed locking |

### 2.6 WAL Hybrid Persistence

Redis uses a combined RDB snapshot + AOF append-only log for persistence. We chose a different architecture: a local WAL for immediate durability paired with asynchronous streaming to object storage (S3/local filesystem) for long-term backup.

```
Client request → ShardActor.execute(cmd) → (result, delta)
                                              ↓
                                    WalActor.write(delta) → group commit → fsync
                                              ↓
                                    Response to client (data is now durable)
                                              ↓ (async, non-blocking)
                                    DeltaSink → PersistenceActor → ObjectStore
                                              ↓ (after successful stream)
                                    WalActor.truncate(streamed_timestamp)
```

The WAL actor implements turbopuffer-inspired group commit: concurrent writers append entries to a bounded buffer, then a single `fsync` flushes the entire batch to disk before resolving all waiters. With 50 concurrent clients, this amortizes the ~100μs fsync cost to ~2μs per write.

Three fsync policies mirror Redis AOF `appendfsync` options: `Always` (group commit before ack, RPO=0), `EverySecond` (ack before fsync, RPO≤1s), and `No` (OS-managed flush, unbounded RPO). Recovery loads from the object store first (checkpoint + segments), determines the high-water mark, then replays WAL entries at or above that mark. CRDT idempotency makes duplicate replay safe, eliminating the need for exactly-once tracking.

The WAL file format uses 16-byte entry overhead (data length + Lamport timestamp + CRC32 checksum). Per-entry checksumming means a torn write from a crash corrupts only the last entry; the reader stops at the first corrupt entry and recovers everything before it.

---

## 3. Verification Methodology

### 3.1 The Core Principle

AI-generated code must be verified by methods with objective, mechanical acceptance criteria.

This principle is more nuanced than "verify by something Claude didn't write." In this project, Claude wrote both the implementation *and* much of the verification code -- the TLA+ specs, the Stateright models, the DST harnesses. If the criterion were merely "different author," we would have a problem: the same model that misunderstands a protocol could write a TLA+ spec that encodes the same misunderstanding.

The escape from this trap is not *who* writes the verification, but *what kind* of verification it is. Each layer in our pyramid was chosen because it has **mechanical pass/fail criteria that are independent of the author's intent**:

- **TLA+ / TLC**: The model checker exhaustively enumerates every reachable state. If an invariant is wrong (too weak), it will not catch bugs -- but it cannot produce false positives. A spec that passes TLC with correct invariants provides a mathematical guarantee, regardless of who wrote it.
- **Stateright**: Same principle as TLC but in Rust. Exhaustive BFS of the state space. The checker does not know or care who authored the model.
- **Redis Tcl suite**: Written by the Redis project maintainers. Neither the human nor Claude had any role in creating these tests. They exercise edge cases that come from years of production Redis usage.
- **Maelstrom/Knossos**: Kyle Kingsbury's linearizability checker applies a formal consistency model to operation histories. The checker is mathematically rigorous -- it either finds a valid linearization or it doesn't.
- **DST with shadow state**: The shadow model is trivially simple (a HashMap). The invariant is mechanical: does the real implementation produce the same output as the HashMap for every operation? A subtly wrong shadow model would cause false positives (passing tests for buggy code), but the shadow is simple enough to inspect by hand.

The key insight: **the verification methods have objective criteria that do not depend on the correctness of the author's mental model.** A wrong TLA+ spec will pass TLC but fail to catch bugs in the implementation -- which the Tcl suite or DST would then catch. A wrong DST shadow would miss bugs -- which Stateright's exhaustive search would catch. The layers are complementary precisely because they have independent failure modes.

```
  ┌─────────────────────────────────────────────┐
  │           Verification Pyramid              │
  │                                             │
  │              ╱╲    TLA+ specs               │  Protocol bugs
  │             ╱  ╲   (exhaustive TLC)         │
  │            ╱────╲                           │
  │           ╱      ╲  Stateright              │  State-space bugs
  │          ╱        ╲ (exhaustive BFS)        │
  │         ╱──────────╲                        │
  │        ╱            ╲ Maelstrom/Jepsen      │  Consistency bugs
  │       ╱              ╲(linearizability)     │
  │      ╱────────────────╲                     │
  │     ╱    Redis Tcl     ╲ WAL DST (crash     │  Semantic + durability
  │    ╱     Suite          ╲ + fault inject)   │  bugs
  │   ╱──────────────────────╲                  │
  │  ╱      Unit + DST        ╲                 │  Implementation bugs
  │ ╱       (fault injection)  ╲                │
  │╱────────────────────────────╲               │
  └─────────────────────────────────────────────┘
    More tests, faster          Fewer, slower, deeper
```

### 3.2 Layer 1: Deterministic Simulation Testing

The first layer draws directly from FoundationDB's simulation testing philosophy (also adopted by TigerBeetle and Antithesis): replace all sources of nondeterminism with controlled abstractions, then run thousands of randomized scenarios from fixed seeds.

**Controlled time and randomness.** `VirtualTime` replaces wall-clock time throughout the simulation path -- a monotonic u64 of milliseconds, advanced explicitly by the harness. `SimulatedRng` provides a deterministic PRNG seeded from a u64. Given seed 42, the ten-thousandth random number is always the same, across platforms, across runs. A failing test prints its seed; re-running with that seed reproduces the exact same execution.

**Fault injection.** The `buggify` module, modeled on FoundationDB's BUGGIFY macro, defines injectable faults across six categories: network (packet drop, corruption, reordering, delay, connection reset, timeout, duplicate), timer (clock drift, skips, jumps), process (crash, pause, OOM, CPU starvation), disk (write failure, corruption, fsync failure, stale read, disk full), object store (put/get/delete failure, corruption, timeout, partial write), and replication (gossip drop, delay, corruption, split brain, stale replica). Three preset profiles -- calm (0.1x multiplier), moderate (1x), and chaos (3x) -- control overall aggression.

```
  DST Harness Flow (per seed):

  ┌──────────┐     ┌──────────────┐     ┌───────────────┐
  │ Seed: 42 │────▶│ SimulatedRng │────▶│ Generate      │
  └──────────┘     │ VirtualTime  │     │ random ops    │
                   └──────────────┘     └───────┬───────┘
                                                │
                          ┌─────────────────────┼─────────────────────┐
                          ▼                     ▼                     ▼
                   ┌──────────────┐      ┌──────────────┐     ┌──────────────┐
                   │  Executor    │      │   Shadow     │     │   Buggify    │
                   │  (real impl) │      │   (HashMap)  │     │   (faults)   │
                   └──────┬───────┘      └──────┬───────┘     └──────────────┘
                          │                     │
                          ▼                     ▼
                   ┌──────────────────────────────────┐
                   │  Compare response + state after  │
                   │  EVERY operation. Mismatch =     │
                   │  invariant violation.             │
                   └──────────────────────────────────┘
```

**The executor DST harness** maintains a shadow state -- a reference model implemented as a simple HashMap -- alongside the real `CommandExecutor`. After every operation, the harness compares the executor's RESP response against the expected value computed from the shadow.

**The connection-level transaction DST** operates on a single `CommandExecutor`, interleaving commands from two simulated client perspectives to exercise WATCH conflict detection, MULTI/EXEC queuing, and DISCARD error handling.

**The CRDT convergence DST** tests four data structures -- GCounter, PNCounter, ORSet, and VectorClock -- under partition injection and message loss. Each harness creates multiple replicas, applies random operations, runs pairwise sync rounds (with configurable message drop probability), and asserts that all replicas converge to identical state.

**The WAL DST harness** verifies durability guarantees under crash and disk fault injection. A `SimulatedWalStore` wraps the in-memory store with buggify fault injection (WRITE_FAIL, PARTIAL_WRITE, FSYNC_FAIL, CORRUPTION, DISK_FULL). The harness writes deltas, tracks which were acknowledged (fsync returned success), simulates a crash at a randomized point by truncating all files to their last synced position, then recovers and verifies every acknowledged write is present. The key invariant: in `Always` fsync mode, zero acknowledged writes may be lost.

### 3.3 Layer 2: Redis Tcl Compatibility

The official Redis test suite, maintained by the Redis authors, is run unmodified against the Rust implementation. This is the strongest form of external verification: the tests were written by people who had no knowledge of this project, and Claude had no role in creating them.

The Tcl harness terminates a test file on the first unimplemented command. Every test that runs against an implemented command passes. The remaining failures are from unimplemented commands, not behavioral bugs.

A critical property: error message strings must match Redis's exact format, since the Tcl tests use glob assertions like `assert_error "*wrong number of arguments*"`. This caught several formatting bugs that the DST harness, which checks error *types* rather than error *strings*, missed entirely.

### 3.4 Layer 3: Maelstrom/Jepsen Linearizability

The third layer uses Kyle Kingsbury's Maelstrom workbench -- a teaching tool built on the Jepsen testing library that reuses the Knossos linearizability checker. The CI pipeline runs 1-node baseline and 5-node stress configurations.

What this proves: under Maelstrom's simulated network (reliable, near-instant delivery), the gossip protocol converges fast enough that no linearizability violation is observable at the workload level. What it does not prove: linearizability under real network conditions. The system uses eventual consistency by design; under sustained partitions, reads may return stale values.

### 3.5 Layer 4: Formal Methods

**Stateright model checking** enumerates all possible interleavings of operations across replicas and persistence states, verifying that properties hold in *every* reachable state. Three models are checked exhaustively: `CrdtMergeModel` (Lamport clock monotonicity, tombstone consistency, timestamp validity under concurrent operations), `WriteBufferModel` (buffer bounds, segment ID monotonicity, manifest consistency), and `WalDurabilityModel` (truncation safety, recovery completeness, buffer-not-acknowledged before sync, high-water mark consistency). The WAL model explores partial streaming intermediate states (one entry streamed per action, not all-at-once), ensuring that truncation bugs hiding in partial-streaming states are caught.

**TLA+ specifications** formalize the distributed protocols: `ReplicationConvergence.tla` (LWW merge under partitions), `GossipProtocol.tla` (delta dissemination), `AntiEntropy.tla` (Merkle-tree sync), `StreamingPersistence.tla` (write buffer durability), and `WalDurability.tla` (WAL group commit, fsync policies, truncation safety, crash recovery with object store high-water mark). Key TLA+ invariants map to concrete Stateright properties and runtime assertions, particularly for persistence and WAL durability.

### 3.6 CI Integration

The CI pipeline runs on every push and pull request. The main `build-and-test` job runs all tests (unit + DST integration suites), Tcl compatibility, and Maelstrom linearizability. A `clippy` job enforces zero warnings. Stateright exhaustive model checking runs in a separate workflow because state-space exploration can take hours depending on model bounds. A `DST Soak` workflow is available on demand for extended fault injection runs. TLA+ specifications are checked manually with the TLC model checker. Every PR must pass all automated layers; a failure in any layer blocks the merge.

---

## 4. Results

### 4.1 Compatibility

Every test that runs against an implemented command passes. Fully passing suites include `unit/type/incr` and `unit/expire`. The `unit/type/string` suite passes all tests until reaching LCS (not implemented). The `unit/multi` suite passes until reaching SWAPDB (database swapping not implemented).

### 4.2 Performance

**Throughput Comparison (Docker, 2 CPUs, 1 GB RAM, 50 clients, `redis-benchmark`)**

Results vary across runs due to Docker scheduling and host load. The table below shows representative numbers from a single run; we observed 2x variance across repeated runs on the same hardware. The ranges in the Relative column reflect the spread across multiple runs.

| | Redis 7.4 | redis-rust | Relative (range) |
|---|-----------|------------|----------|
| SET P=1 | 154--197K rps | 146--186K rps | 81--99% |
| GET P=1 | 152--188K rps | 140--183K rps | 77--97% |
| SET P=16 | 1.0--1.4M rps | 775K--1.1M rps | 75--87% |
| GET P=16 | 917K--1.5M rps | 709K--1.2M rps | 73--101% |

At pipeline depth 1, SET throughput is typically within 5--10% of Redis 7.4. GET lags by up to 23%, likely due to response serialization overhead. Under pipelining (P=16), both operations show wider variance, with some runs showing redis-rust matching or exceeding Redis depending on host scheduling.

### 4.3 Correctness

Maelstrom/Knossos found valid linearizable orderings at 1-node and 5-node scales under simulated network conditions. The high CAS failure rate at multi-node scales is expected -- CRDT gossip means compare-and-swap frequently reads stale values. As noted in Section 3.4, linearizability violations under higher load are correct behavior for an eventually consistent system. The CI pipeline tolerates these but fails on exceptions, crashes, or protocol errors.

---

## 5. Lessons Learned

### 5.1 What Worked

**Claude excels at boilerplate and pattern replication.** Once one Redis command was implemented end-to-end, Claude could replicate the pattern across dozens of similar commands with high accuracy. The mechanical aspects of adding a new command -- updating match arms, writing argument validation, encoding RESP responses -- are exactly the kind of repetitive work where LLM agents perform well.

**DST and verification harnesses caught real bugs Claude introduced.** Six production-grade bugs were discovered through the verification process: (1) LPOP, RPOP, SREM, SPOP, HDEL, and ZREM failed to delete keys when collections became empty; (2) MSET's postcondition assertion checked all key-value pairs including duplicates instead of the last value per key; (3) WATCH inside MULTI was being queued rather than returning an immediate error; (4) SETRANGE on a non-existing key with an empty value created an empty key instead of returning 0; (5) SET...GET on a wrong-type key didn't return WRONGTYPE before modifying the key; (6) DBSIZE shadow state drifted from the executor due to lazy vs eager expiration. None of these would have been caught by the Tcl suite alone.

**Independent Claude review found additional issues.** A separate Claude session (fresh context, no implementation history) identified additional bugs: fast-path bypassing MULTI/EXEC, multi-key DEL only routing to the first shard, unchecked integer overflow in SETRANGE, and INCRBYFLOAT accepting NaN/Infinity at parse time.

### 5.2 What Didn't Work

**Claude tends toward over-engineering.** Left unprompted, Claude introduced abstractions, trait hierarchies, and configuration layers that nobody asked for. The human's role was often to delete code rather than write it.

**Error message format compatibility required iterative debugging.** Getting strings like `ERR wrong number of arguments for 'xxx' command` exactly right -- including capitalization, punctuation, and the single quotes around the command name -- required running the Tcl harness repeatedly. Claude produced plausible but slightly wrong error strings and had no way to detect the mismatch without external feedback.

**Claude doesn't naturally write TigerStyle assertions.** Precondition and postcondition `debug_assert!` calls, `verify_invariants()` methods, and checked arithmetic had to be explicitly requested -- repeatedly. Claude's default mode is to produce code that works, not code that proves it works.

**Large structural changes required human architectural reasoning.** Moving MULTI/EXEC state from the per-shard executor to the connection level was a cross-cutting change that Claude could not plan or execute autonomously. It required understanding the interaction between connection lifecycle, shard routing, and transaction isolation.

### 5.3 The Verification Harness as the Key Insight

The most important artifact in this project is not the Redis implementation. It is the verification harness.

Without it, we would have a Redis-like system that appears to work but whose correctness is asserted only by the same model that wrote it. With it, every claim is backed by a runnable command and an expected output. The verification code is itself Claude-generated -- but the distinction that matters is not *who* wrote the tests but whether the tests have objective pass/fail criteria independent of the author's understanding (as argued in Section 3.1).

As model-generated code becomes more common, the bottleneck shifts from writing code to verifying code. A well-designed verification harness turns a model from an unaudited author into a supervised contributor whose output can be mechanically checked. **The harness is the trust boundary.**

---

## 6. Conclusion and Future Work

This project is a case study in human-Claude systems programming with rigorous verification, not a production Redis replacement. It demonstrates that Claude can co-author a functional, reasonably performant Redis-compatible server -- with 77--99% of Redis 7.4 throughput, passing the official test suite for implemented commands, with DST-verified durable persistence -- when paired with a human engineer who provides architectural direction and a verification harness that catches Claude's mistakes.

The verification methodology is the main contribution. The multi-layer pyramid forms a pipeline that makes model-generated systems code auditable. The six bugs caught by DST (Section 5.1) are the kinds of subtle correctness issues that pass code review and unit tests but fail under adversarial workloads.

The WAL hybrid persistence layer demonstrates the methodology applied to a durability subsystem. The key invariant -- every acknowledged write survives crash+recovery in `Always` fsync mode -- is verified at multiple levels: TLA+ proves the protocol correct, Stateright exhaustively checks state interleavings, and DST runs 150+ seeds with simulated disk faults and crash injection (plus a 1000-seed stress test available for manual runs). Docker integration tests confirm zero data loss across multiple SIGKILL crashes.

Future work includes expanding Tcl suite coverage (LCS, blocking operations are the next frontier), extending WAL persistence to cover lists, sets, and sorted sets (currently only string and hash types produce replication deltas), and exploring cluster-mode sharding across multiple machines. A particularly interesting direction is closing the loop between verification and production: connecting the server to observability systems like Datadog to feed real-world performance data, error rates, and latency distributions back into the development cycle. The project already includes optional Datadog integration (metrics, tracing, and logging via feature flag). The vision is a feedback loop where production telemetry informs which commands to optimize, which edge cases to harden, and which verification layers need strengthening -- turning observability into an additional verification layer that operates on real traffic rather than synthetic workloads.

On the verification side, the open question is whether this methodology scales: as model capabilities improve and the generated code grows more complex, does the current verification pyramid remain sufficient, or does the harness itself need to evolve?

We do not yet have an answer, and we are skeptical of anyone who claims to.

---

**Source code:** https://github.com/nerdsane/redis-rust

**Verification harness:** See `docs/HARNESS.md` in the repository for runnable commands and expected outputs.
