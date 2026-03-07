# redis-rust: A Case Study in AI-Assisted Systems Programming with Deterministic Verification

**Authors:** Sesh Nalla and Claude Code (Anthropic, Opus 4.5 → Opus 4.6)

**Abstract:**

AI models continue to improve in their ability to generate code. Software engineers now create and submit large, complex AI-generated codebases for review and integration into production software. While these codebases may compile and appear to perform, senior engineers need to learn how to verify that these codebases are correct. This is especially difficult in distributed systems, where edge cases can appear only under very specific timing conditions not apparent in a rudimentary test suite.

To test the current capabilities of AI Agents creating a distributed system, we chose to have Opus 4.6 produce an implementation of Redis in Rust. This case study proposes a reusable verification methodology, a "verification pyramid" that establishes correctness through multiple layers. We then compare the agent generated Rust implementation to the official Redis 7.4 distribution.

---

## 1. Background

### 1.1 Coding Agent History

From 2021 to 2024, there was a slow progression of LLM model's ability to generate code. IDE integration was possible such as with Github Copilot, which provided line completion, file scaffolding, and some generation of common patterns. However, this code tended to be rudimentary, localized to common use cases, overly verbose, and highly prone to error.

In 2024, developers still typically interacted with a browser interface, creating a layer of separation between the code generated and the developer's workspace.

This changed with the release of Anthropic's Claude Code, integrating LLMs into the terminal with a set of tools to directly read, write, and execute code in the codebases that an engineer works in. 2025 then saw a rapid progression in a models' ability to produce quality code.

Anthropic's Claude Opus 4.5's launch in November represented a huge shift in a coding to create not just code, but complex software systems. From within the Claude Code "harness", Opus 4.5 could sustain reasoning about implementations across thousands of lines, hold architectural context across a code base, and consider aspects such as module boundaries, memory safety, and concurrent access.

The issue was no longer whether coding agents could write compilable code, but the limits to complexity and quality that an agent could achieve. And to generate code that doesn't just look right, but performs correctly under all known testing. This is the area this paper seeks to explore.

### 1.2 Problems in Testing Agent Code

AI-generated code must be verified by methods with objective pass/fail acceptance criteria. This has proved problematic.

When creating a sufficiently complex system, a human writing and designing each test without AI assistance would often eliminate most of the benefit of using the AI assistant in the first place.

However, coding agents follow a core "agentic-loop", they:
- Gather information about the current and desired state
- Take action to reach the desired state
- Attempt to verify the desired state has been reached
- Loop until the desired state is reached

A coding agent is incentivized to pass its verification step, so it sometimes makes "bad-faith" attempts to do so. Agents often manipulate source code and test code together, "hard-coding" them so that tests will pass, despite an application failing the behavior tested in practice.

A common area of recent research is encoding tests into "black boxes", tests stored in a location or layer of obfuscation that an Agent does not have access to.

This paper suggests still having the Agent write some or most of the verification code, but in formats more conducive to creating mechanical pass/fail criteria that are independent of the author's (human or AI) intent.

### 1.3 Problem Statement

We sought to test the coding agent to create a codebase that would be difficult to understand while still possible for a human to verify. For this purpose we chose a distributed system.

Distributed systems require reasoning about concurrency, partial failure, network nondeterminism, and subtle invariants that hold across multiple machines. They are also difficult to completely test, errors in application logic can appear only in specific timing conditions.

### 1.3.1 Target Application

This case study attempts to have an agent create a subset of Redis functionality in Rust, `redis-rust`. To keep the experiment within an achievable but significant scope, we implement only a useful subset (strings, lists, sets, hashes, sorted sets, transactions, expiration, Lua scripting).

Redis was chosen for the following characteristics:

- Redis' command set is well-documented, widely understood, but has known edge cases to test for.
- There is an official Redis test suite that checks behavior at a granular level of detail. This prohibits an AI from writing false tests in order to pass them.
- Redis has a standard `redis-benchmark`. The same benchmarks will be run on both `redis-rust` and Redis 7.4.

### 1.4 Contributions

The primary contribution of this paper is the proposal of a verification methodology for AI-assisted system code, a **verification pyramid**.

The paper also functions as a case study:

- It documents the emergent design passes that the human-agent team worked through
- It presents the final performance, feature, and test suite characteristics of `redis-rust`

## 2. Verification Methodology

The verification pyramid is a definition of layers of defense against common pitfalls of AI-generated code:

- Plausible implementations with subtle errors
- Correct logic with incorrect error messages
- Regressions under system load

The layers of the pyramid are each made of objective, mechanical pass/fail criteria that are independent of the author's intent to pass them.

This project uses the following methods:

- TLA+/TLC: A spec that passes TLC with correct invariants provides a mathematical guarantee, regardless of who wrote it.
- Stateright: A rust library that tests "all possible observability behaviors within a specification". (From Stateright documentation)
- Maelstrom/Knossos: A linearizability checker that applies a formal consistency model to operation histories.
- Redis Tcl suite: An exhaustive suite of tests written by the Redis project maintainers.
- A simple HashMap with shadow state: checks whether the real implementation matches the Hashmap entry for the same input.
- Deterministic Simulation Testing: replaces all nondeterministic sources of input with structured abstractions, then runs randomized scenarios from fixed seeds.

Each verification method has objective criteria that do not depend on the correctness of the author's mental model. Each layer catches errors that the previous might miss:

- A wrong TLA+ spec can pass TLC, but can fail to catch bugs in the spec's implementation
- The Tcl or HashMap suite catches more of the implementation errors, but may lack completeness
- Stateright's exhaustive search would catch any gaps in the previous step

As the testing moves down the pyramid, the complementary tests form a complete set of all possible application errors.

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

### 2.1 Layer 1: Deterministic Simulation Testing

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

### 2.2 Layer 2: Redis Tcl Compatibility

The official Redis test suite, maintained by the Redis authors, is run unmodified against the Rust implementation. This is the strongest form of external verification: the tests were written by people who had no knowledge of this project, and Claude had no role in creating them.

The Tcl harness terminates a test file on the first unimplemented command. Every test that runs against an implemented command passes. The remaining failures are from unimplemented commands, not behavioral bugs.

A critical property: error message strings must match Redis's exact format, since the Tcl tests use glob assertions like `assert_error "*wrong number of arguments*"`. This caught several formatting bugs that the DST harness, which checks error *types* rather than error *strings*, missed entirely.

### 2.3 Layer 3: Maelstrom/Jepsen Linearizability

The third layer uses Kyle Kingsbury's Maelstrom workbench -- a teaching tool built on the Jepsen testing library that reuses the Knossos linearizability checker. The CI pipeline runs 1-node baseline and 5-node stress configurations.

What this proves: under Maelstrom's simulated network (reliable, near-instant delivery), the gossip protocol converges fast enough that no linearizability violation is observable at the workload level. What it does not prove: linearizability under real network conditions. The system uses eventual consistency by design; under sustained partitions, reads may return stale values.

### 2.4 Layer 4: Formal Methods

**Stateright model checking** enumerates all possible interleavings of operations across replicas and persistence states, verifying that properties hold in *every* reachable state. Three models are checked exhaustively: `CrdtMergeModel` (Lamport clock monotonicity, tombstone consistency, timestamp validity under concurrent operations), `WriteBufferModel` (buffer bounds, segment ID monotonicity, manifest consistency), and `WalDurabilityModel` (truncation safety, recovery completeness, buffer-not-acknowledged before sync, high-water mark consistency). The WAL model explores partial streaming intermediate states (one entry streamed per action, not all-at-once), ensuring that truncation bugs hiding in partial-streaming states are caught.

**TLA+ specifications** formalize the distributed protocols: `ReplicationConvergence.tla` (LWW merge under partitions), `GossipProtocol.tla` (delta dissemination), `AntiEntropy.tla` (Merkle-tree sync), `StreamingPersistence.tla` (write buffer durability), and `WalDurability.tla` (WAL group commit, fsync policies, truncation safety, crash recovery with object store high-water mark). Key TLA+ invariants map to concrete Stateright properties and runtime assertions, particularly for persistence and WAL durability.

### 2.5 CI Integration

The CI pipeline runs on every push and pull request. The main `build-and-test` job runs all tests (unit + DST integration suites), Tcl compatibility, and Maelstrom linearizability. A `clippy` job enforces zero warnings. Stateright exhaustive model checking runs in a separate workflow because state-space exploration can take hours depending on model bounds. A `DST Soak` workflow is available on demand for extended fault injection runs. TLA+ specifications are checked manually with the TLC model checker. Every PR must pass all automated layers; a failure in any layer blocks the merge.

---

## 3. Architecture

### 3.1 Redis Background

Each data object stored in Redis has a unique **key**. To modify or retrieve a data object, a command must pass that objects key. The full set of possible keys in the Redis store is the **keyspace**.

### 3.2 Actor-per-Shard Design

The `redis-rust` server partitions the keyspace across a variable number of shards, which are configurable at startup.

Each shard runs on its own thread, a `tokio` task and is made up of two components exclusive to that shard:

- A `CommandExecutor`, responsible for translating the command into a data store operation
- A `HashMap` functioning as that shard's data storage.

Shards can communicate with each other through unbounded channels, but each shard exclusively owns its own data, values do not change storage locations from shard to shard. For this reason, there are no locks within the per-shard data storage.

### 3.3 Cache Efficiency

Each shard's `Hashmap` fits in one partition of a CPU cache partition, reducing the need to swap values in and out of the cache. This results in near instantaneous read/write speeds for existing values.

### 3.4 Single-Thread Execution per Shard

Because the `CommandExecutable` is always on a single thread, all operations on a given key require no cross-thread coordination.

### 3.5 GET/SET Fast Path

The majority of Redis operations tend to be `GET` or `SET`, so it is critical for these operations to be as fast as possible. `redis-rust` allows for a more efficient bypass for these operations, parsing bytes directly without enum creation or an extra string copy.

### 3.6 Connection-Level Transactions

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

### 3.7 CRDT Replication

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

### 3.8 Scaling Behavior

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

### 3.9 Explicit Trade-offs

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

### 3.10 WAL Hybrid Persistence

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

Maelstrom/Knossos found valid linearizable orderings at 1-node and 5-node scales under simulated network conditions. The high CAS failure rate at multi-node scales is expected -- CRDT gossip means compare-and-swap frequently reads stale values. As noted in Section 2.3, linearizability violations under higher load are correct behavior for an eventually consistent system. The CI pipeline tolerates these but fails on exceptions, crashes, or protocol errors.

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

Without it, we would have a Redis-like system that appears to work but whose correctness is asserted only by the same model that wrote it. With it, every claim is backed by a runnable command and an expected output. The verification code is itself Claude-generated -- but the distinction that matters is not *who* wrote the tests but whether the tests have objective pass/fail criteria independent of the author's understanding (as argued in Section 2.1).

As model-generated code becomes more common, the bottleneck shifts from writing code to verifying code. A well-designed verification harness turns a model from an unaudited author into a supervised contributor whose output can be mechanically checked. **The harness is the trust boundary.**

---

## 6. Conclusion and Future Work

This project is a case study in human-Claude systems programming with rigorous verification. It demonstrates that Claude can co-author a functional, reasonably performant Redis-compatible server -- with 77--99% of Redis 7.4 throughput, passing the official test suite for implemented commands, with DST-verified durable persistence -- when paired with a human engineer who provides architectural direction and a verification harness that catches Claude's mistakes.

The verification methodology is the main contribution. The multi-layer pyramid forms a pipeline that makes model-generated systems code auditable. The six bugs caught by DST (Section 5.1) are the kinds of subtle correctness issues that pass code review and unit tests but fail under adversarial workloads.

The WAL hybrid persistence layer demonstrates the methodology applied to a durability subsystem. The key invariant -- every acknowledged write survives crash+recovery in `Always` fsync mode -- is verified at multiple levels: TLA+ proves the protocol correct, Stateright exhaustively checks state interleavings, and DST runs 150+ seeds with simulated disk faults and crash injection (plus a 1000-seed stress test available for manual runs). Docker integration tests confirm zero data loss across multiple SIGKILL crashes.

Future work includes expanding Tcl suite coverage (LCS, blocking operations are the next frontier), extending WAL persistence to cover lists, sets, and sorted sets (currently only string and hash types produce replication deltas), and exploring cluster-mode sharding across multiple machines. A particularly interesting direction is closing the loop between verification and production: connecting the server to observability systems like Datadog to feed real-world performance data, error rates, and latency distributions back into the development cycle. The project already includes optional Datadog integration (metrics, tracing, and logging via feature flag). The vision is a feedback loop where production telemetry informs which commands to optimize, which edge cases to harden, and which verification layers need strengthening -- turning observability into an additional verification layer that operates on real traffic rather than synthetic workloads.

On the verification side, the open question is whether this methodology scales: as model capabilities improve and the generated code grows more complex, does the current verification pyramid remain sufficient, or does the harness itself need to evolve?

We do not yet have an answer.

---

**Source code:** https://github.com/nerdsane/redis-rust

**Verification harness:** See `docs/HARNESS.md` in the repository for runnable commands and expected outputs.
