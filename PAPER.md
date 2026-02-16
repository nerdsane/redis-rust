# redis-rust: A Case Study in AI-Assisted Systems Programming with Deterministic Verification

**Authors:** Sesh Nalla and Claude Code (Anthropic, Opus 4.5 → Opus 4.6)

**Abstract:** We describe redis-rust, an experimental Redis-compatible in-memory data store written in Rust and co-authored by a human systems engineer and Claude Code (Anthropic, Opus 4.5 → Opus 4.6). The project began as a deliberate test: after a step-change in AI code generation capability in late 2025, could human-AI collaboration produce a distributed systems implementation that is not merely plausible but verifiably correct? The system implements 75+ Redis commands across strings, lists, sets, hashes, sorted sets, transactions, Lua scripting, and key expiration, with CRDT-based multi-node replication using gossip and anti-entropy protocols. Correctness is established through a 4-layer verification harness: 507 deterministic unit and simulation tests (including fault-injected CRDT convergence under network partitions), the official Redis Tcl test suite (28/28 incr, all expire tests passing), 5-node Maelstrom/Jepsen linearizability checking, and Docker-based performance benchmarking. On equivalent hardware, throughput reaches 80--99% of Redis 7.4. This is not a production Redis replacement. It is an honest case study in what AI-assisted systems programming can and cannot do today, with a reusable verification methodology as its primary artifact.

---

## 1. Introduction

### 1.1 Genesis

In late 2025, Anthropic released Claude Opus 4.5 -- a model that crossed a threshold in code generation qualitatively different from what came before. Prior models could autocomplete functions and generate plausible-looking snippets. Opus 4.5, which became the first AI model to break 80% on SWE-bench Verified, could sustain coherent implementations across thousands of lines, maintain internal consistency across module boundaries, hold architectural context across an entire codebase, and reason about subtle correctness properties like memory safety, concurrent access, and distributed protocol invariants. It could read a TLA+ spec and produce a Rust implementation that preserved the spec's invariants. It could be asked to "add WATCH support to MULTI/EXEC across a sharded architecture" and produce a design that correctly identified the value-snapshot approach as the only option that avoids shared mutable state -- then implement it.

We wanted to test this in the hardest way we could think of that was still tractable for two people (one human, one AI). Distributed systems are notoriously difficult. They require reasoning about concurrency, partial failure, network nondeterminism, and subtle invariants that hold across multiple machines. They are also notoriously difficult to *test* -- the interesting bugs only appear under specific timing conditions that unit tests rarely exercise. If AI-assisted development could produce a verifiably correct distributed system, that would be a meaningful data point. If it could not, understanding *where* it failed would be equally valuable.

The question was not "can an AI write code that compiles?" That bar was cleared years ago. The question was: **can a human-AI team produce systems code that survives the same verification methods we would apply to human-written code?** Not code that looks right. Code that *is* right, as far as we can tell, under adversarial testing.

### 1.2 Why Redis

Redis was chosen as the implementation target for several pragmatic reasons:

**Familiar semantics.** Redis's command set is well-documented, widely understood, and has sharp behavioral edges (WRONGTYPE errors, empty-collection auto-deletion, expiration-on-access) that make a faithful implementation non-trivial. It is easy to explain what the system should do. It is hard to get every edge case right.

**An official test suite exists.** The Redis project maintains a Tcl-based test suite that exercises command behavior at a level of detail that no handwritten test suite could match in a reasonable timeframe. Passing these tests provides a confidence signal that is independent of the authors -- neither the human nor the AI wrote the tests, and neither can unconsciously bias them toward the implementation.

**Performance is measurable.** `redis-benchmark` is a standard tool. Running the same benchmark against Redis 7.4 and our implementation on identical hardware gives a concrete, reproducible performance comparison. No synthetic microbenchmarks, no favorable conditions -- just the standard tool with default settings.

**The scope is bounded.** A full Redis reimplementation is enormous, but a useful subset (strings, lists, sets, hashes, sorted sets, transactions, expiration, Lua scripting) is achievable in weeks rather than years. We could reach a meaningful "checkpoint" -- passing official tests, matching performance -- without committing to a multi-year project.

### 1.3 Why This Paper

This paper is not a product announcement. redis-rust is explicitly not production software, and we say so in the first line of its documentation.

What we think is worth sharing is the *process*: how we structured verification to keep an AI collaborator honest, what kinds of bugs the AI introduced and how they were caught, where human judgment was irreplaceable, and where AI acceleration was genuine. The 4-layer verification harness -- deterministic simulation with fault injection, official compatibility tests, Jepsen-style linearizability checking, and performance benchmarking -- is a methodology that could be applied to any AI-assisted systems project.

We also want to be straightforward about the limitations. The system does not implement persistence, pub/sub, streams, or cluster rebalancing. The multi-node replication is eventually consistent by design, not linearizable. The Tcl test suite crashes on unimplemented commands rather than gracefully skipping them, so our "pass rate" reflects command coverage as much as correctness. We believe honest reporting of both successes and gaps is more useful than selective presentation.

### 1.4 Contributions

This paper makes three contributions:

1. **A verification methodology for AI-assisted systems code.** The 4-layer harness (deterministic simulation testing with FoundationDB-style fault injection, official Redis Tcl compatibility suite, Maelstrom/Jepsen linearizability checking, and performance benchmarking) provides defense-in-depth against the specific failure modes of AI-generated code: plausible-looking implementations that are subtly wrong, correct logic with incorrect error messages, and working code that silently regresses under load. Each layer catches different classes of bugs, and none requires trusting the AI that wrote the code.

2. **An architecture case study.** The actor-per-shard design, CRDT-based replication with gossip protocol, and Lua scripting integration demonstrate how a human-AI team navigated real systems design decisions -- lock-free concurrency via message passing, connection-level vs. shard-level transaction state, virtual time for deterministic simulation. We document the trade-offs explicitly, including the ones we got wrong on the first attempt.

3. **An honest accounting of results.** 507 passing tests, 2 fully passing Tcl suites, 5-node Maelstrom validation, and 80--99% of Redis 7.4 throughput. Also: 4 Tcl suites not attempted, no persistence, no pub/sub, no linearizable replication. Three production-grade bugs caught by DST that would have shipped in a less-tested implementation: empty-collection cleanup, MSET postcondition violation, and WATCH-inside-MULTI mishandling. We report what works, what does not, and what we learned from each failure.

---

## 2. Architecture and Trade-offs

### 2.1 Actor-per-Shard Design

The server partitions keyspace across N shards, where each shard is a tokio task that exclusively owns a `CommandExecutor`. The number of shards is configurable at startup via `perf_config.toml` and defaults to the number of available CPU cores. Shard actors communicate only through unbounded mpsc channels -- there are no mutexes, no reader-writer locks, and no shared mutable state within the data path.

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
                           (no locks)
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

The replication layer draws from Anna KVS [Wu et al., 2019]. Each key-value pair is wrapped in a `ReplicatedValue` containing a last-writer-wins register timestamped with a Lamport clock. The CRDT library also includes `GCounter`, `PNCounter`, `ORSet`, `GSet`, and `VectorClock` types, each with merge functions proven commutative and idempotent via Stateright model checking.

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
| Persistence | RDB + AOF | None | Focus on in-memory correctness |
| Blocking ops | BLPOP, BRPOP | Not implemented | Requires cross-shard signaling |
| Bitmaps/Streams | Full support | Not implemented | Low priority for verification research |
| Thread model | Single-threaded + I/O | Actor-per-shard (tokio) | Multi-core scaling without a GIL |
| Transaction scope | All keys visible | Queued at connection, per-shard dispatch | Atomicity from client perspective |
| Cluster mode | Hash slots | Single-node sharding + CRDT gossip | Hash slots require client-side routing |
| Lua key access | All keys | Only executing shard (or single-shard mode) | Multi-shard Lua needs distributed locking |

---

## 3. Verification Methodology

### 3.1 The Core Principle

AI-generated code must be verified by something other than the AI that wrote it.

This principle is the foundation of the project's engineering discipline. When an LLM writes a function, the LLM can also write a test for that function -- but that test will share the same blind spots as the code it verifies. A model that misunderstands Redis's empty-collection deletion semantics will produce both a buggy LPOP implementation and a test that asserts the buggy behavior. The only way to escape this trap is to introduce verification sources that the AI did not author and cannot influence.

```
  ┌─────────────────────────────────────────────┐
  │           Verification Pyramid              │
  │                                             │
  │              ╱╲    TLA+ / Stateright        │  Design bugs
  │             ╱  ╲   (formal methods)         │
  │            ╱────╲                           │
  │           ╱      ╲  Maelstrom/Jepsen        │  Consistency bugs
  │          ╱        ╲ (linearizability)        │
  │         ╱──────────╲                        │
  │        ╱            ╲ Redis Tcl Suite       │  Semantic bugs
  │       ╱              ╲(official, external)  │
  │      ╱────────────────╲                     │
  │     ╱                  ╲ DST + Unit Tests   │  Implementation bugs
  │    ╱    507 tests       ╲(fault injection)  │
  │   ╱──────────────────────╲                  │
  └─────────────────────────────────────────────┘
    More tests, faster          Fewer, slower, deeper
```

### 3.2 Layer 1: Deterministic Simulation Testing

The first layer draws directly from FoundationDB's simulation testing philosophy and TigerBeetle's VOPR approach: replace all sources of nondeterminism with controlled abstractions, then run thousands of randomized scenarios from fixed seeds.

**Controlled time and randomness.** `VirtualTime` replaces wall-clock time throughout the simulation path -- a monotonic u64 of milliseconds, advanced explicitly by the harness. `SimulatedRng` provides a deterministic PRNG seeded from a u64. Given seed 42, the ten-thousandth random number is always the same, across platforms, across runs. A failing test prints its seed; re-running with that seed reproduces the exact same execution.

**Fault injection.** The `buggify` module, modeled on FoundationDB's BUGGIFY macro, defines 41 injectable faults across six categories: network (packet drop, corruption, reordering, delay, connection reset, timeout, duplicate), timer (clock drift, skips, jumps), process (crash, pause, OOM, CPU starvation), disk (write failure, corruption, fsync failure, stale read, disk full), object store (put/get/delete failure, corruption, timeout, partial write), and replication (gossip drop, delay, corruption, split brain, stale replica). Three preset profiles -- calm (0.1x multiplier), moderate (1x), and chaos (3x) -- control overall aggression.

```
  DST Harness Flow (per seed):

  ┌──────────┐     ┌──────────────┐     ┌───────────────┐
  │ Seed: 42 │────▶│ SimulatedRng │────▶│ Generate 500  │
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

**The executor DST harness** maintains a shadow state -- a reference model implemented as a simple HashMap -- alongside the real `CommandExecutor`. After every operation, the harness compares the executor's RESP response against the expected value computed from the shadow. Each seed runs 500 operations; the default batch runs 10 seeds.

**The connection-level transaction DST** operates one layer higher. It instantiates two simulated connections sharing a single `ShardedActorState` and exercises the MULTI/EXEC/WATCH state machine -- the exact code path used in production. Each seed runs 200 randomized scenarios; the full suite runs 100 seeds.

**The CRDT convergence DST** tests four data structures -- GCounter, PNCounter, ORSet, and VectorClock -- under partition injection and message loss. Each harness creates multiple replicas, applies random operations, runs pairwise sync rounds (with configurable message drop probability), and asserts that all replicas converge to identical state. Ten CRDT suites run 100 seeds each.

### 3.3 Layer 2: Redis Tcl Compatibility

The official Redis test suite, maintained by the Redis authors, is run unmodified against the Rust implementation. This is the strongest form of external verification: the tests were written by people who had no knowledge of this project, and the AI had no role in creating them.

Current results: `unit/type/incr` passes 28/28 tests; `unit/expire` passes all tests; `unit/type/string` passes 35/39 (crashing at bitmaps); `unit/multi` passes 20/56 (crashing at SWAPDB). The remaining failures are from unimplemented commands, not behavioral bugs.

A critical property: error message strings must match Redis's exact format, since the Tcl tests use glob assertions like `assert_error "*wrong number of arguments*"`. This caught several formatting bugs that the DST harness, which checks error *types* rather than error *strings*, missed entirely.

### 3.4 Layer 3: Maelstrom/Jepsen Linearizability

The third layer uses Kyle Kingsbury's Maelstrom workbench -- the same framework underlying Jepsen -- run unmodified with the Knossos linearizability checker. The CI pipeline runs 1-node baseline and 5-node stress configurations. Both produce `:workload {:valid? true}` with zero anomalies.

What this proves: under Maelstrom's simulated network (reliable, near-instant delivery), the gossip protocol converges fast enough that no linearizability violation is observable. What it does not prove: linearizability under real network conditions. The system uses eventual consistency by design; under sustained partitions, reads may return stale values.

### 3.5 Layer 4: Formal Methods

**Stateright model checking** enumerates all possible interleavings of Set, Delete, and Sync operations across replicas, verifying that merge is commutative, associative, and idempotent. Because the state space is finite and bounded, Stateright checks *every* reachable state, not a random sample.

**Four TLA+ specifications** formalize the distributed protocols: `ReplicationConvergence.tla` (LWW merge under partitions), `GossipProtocol.tla` (delta dissemination), `AntiEntropy.tla` (Merkle-tree sync), and `StreamingPersistence.tla` (write buffer durability). Each TLA+ invariant maps to a concrete runtime assertion in the Rust code.

### 3.6 CI Integration

The CI pipeline runs three layers in a single job on every push and every pull request: unit tests (which include Stateright model checking), Tcl compatibility, and Maelstrom linearizability. TLA+ specifications are checked manually with the TLC model checker; they are not part of automated CI. Every PR must pass all automated layers. There are no `#[ignore]` annotations, no `--skip` flags, no optional test suites. A failure in any layer blocks the merge.

---

## 4. Results

### 4.1 Compatibility

**Table 1: Tcl Compatibility Test Results**

| Suite | Pass | Blocker |
|-------|------|---------|
| `unit/type/incr` | 28/28 | None |
| `unit/expire` | All pass | None |
| `unit/type/string` | 35/39 | SETBIT (bitmaps not implemented) |
| `unit/multi` | 20/56 | SWAPDB (database swapping not implemented) |

The Tcl harness terminates a test file on the first unimplemented command. Every test that runs against an implemented command passes.

### 4.2 Performance

**Table 2: Throughput Comparison (Docker, 2 CPUs, 1 GB RAM, 50 clients)**

| | Redis 7.4 | redis-rust | Relative |
|---|-----------|------------|----------|
| SET P=1 | 148K rps | 147K rps | 99% |
| GET P=1 | 154K rps | 119K rps | 77% |
| SET P=16 | 1.02M rps | 813K rps | 80% |
| GET P=16 | 840K rps | 709K rps | 84% |

At pipeline depth 1, SET throughput is within noise of Redis 7.4. GET lags at 77%, likely due to response serialization overhead. Under pipelining (P=16), the gap narrows for GET (84%) but widens slightly for SET (80%), suggesting the batch-processing path has room for optimization.

### 4.3 Correctness

**Table 3: Maelstrom Linearizability Results**

| Nodes | Operations | Reads | Writes | CAS | Linearizable | Anomalies |
|-------|-----------|-------|--------|-----|-------------|-----------|
| 1 | ~150 | all ok | all ok | all ok | valid | 0 |
| 3 | 190 | 98/98 ok | 29/29 ok | 13/63 ok | valid | 0 |
| 5 | 1,301 | 670/677 ok | 201/201 ok | 80/423 ok | valid | 0 |

The high CAS failure rate at 3 and 5 nodes is expected -- the system uses eventual consistency via CRDT gossip, so compare-and-swap frequently reads stale values. Knossos found valid linearizable orderings at all scales, meaning gossip propagated fast enough under Maelstrom's simulated network that no consistency violations were observable.

We want to be precise about what this does and does not prove. Maelstrom uses a simulated network with reliable, near-instant message delivery. Under low load, the gossip protocol converges quickly enough to produce linearizable histories. Under higher load or slower execution environments, Knossos finds linearizability violations where reads arrive before gossip propagates writes. **These violations are correct behavior** — the system provides eventual consistency, not linearizability. The CI pipeline tolerates linearizability violations but fails on exceptions, crashes, or protocol errors, ensuring the gossip implementation is functionally correct even when convergence timing produces non-linearizable histories.

---

## 5. Lessons Learned

### 5.1 What Worked

**AI excels at boilerplate and pattern replication.** Once one Redis command was implemented end-to-end, the AI could replicate the pattern across dozens of similar commands with high accuracy. The mechanical aspects of adding a new command -- updating match arms, writing argument validation, encoding RESP responses -- are exactly the kind of repetitive work where AI agents perform well.

**DST and verification harnesses caught real bugs the AI introduced.** Three production-grade bugs were discovered by deterministic simulation testing: (1) LPOP, RPOP, SREM, SPOP, HDEL, and ZREM failed to delete keys when collections became empty; (2) MSET's postcondition assertion checked all key-value pairs including duplicates instead of the last value per key; (3) WATCH inside MULTI was being queued rather than returning an immediate error. None of these would have been caught by the Tcl suite alone.

**Independent AI code review found additional issues.** A separate AI review pass (by a different agent with no context on the implementation) identified 6 critical bugs: fast-path bypassing MULTI/EXEC, multi-key DEL only routing to the first shard, unchecked integer overflow in SETRANGE, and INCRBYFLOAT accepting NaN/Infinity at parse time.

### 5.2 What Didn't Work

**AI tends toward over-engineering.** Left unprompted, the AI introduced abstractions, trait hierarchies, and configuration layers that nobody asked for. The human's role was often to delete code rather than write it.

**Error message format compatibility required iterative debugging.** Getting strings like `ERR wrong number of arguments for 'xxx' command` exactly right -- including capitalization, punctuation, and the single quotes around the command name -- required running the Tcl harness repeatedly. The AI produced plausible but slightly wrong error strings and had no way to detect the mismatch without external feedback.

**The AI doesn't naturally write TigerStyle assertions.** Precondition and postcondition `debug_assert!` calls, `verify_invariants()` methods, and checked arithmetic had to be explicitly requested -- repeatedly. The AI's default mode is to produce code that works, not code that proves it works.

**Large structural changes required human architectural reasoning.** Moving MULTI/EXEC state from the per-shard executor to the connection level was a cross-cutting change that the AI could not plan or execute autonomously. It required understanding the interaction between connection lifecycle, shard routing, and transaction isolation.

### 5.3 The Verification Harness as the Key Insight

The most important artifact in this project is not the Redis implementation. It is the verification harness.

The harness makes AI-generated code auditable. Without it, we would have a Redis-like system that appears to work but whose correctness is asserted only by the same agent that wrote it. With it, every claim is backed by a runnable command and an expected output. The DST layer subjects the code to thousands of randomized operation sequences under fault injection, comparing against a shadow model after every operation.

This matters beyond this project. As AI-generated code becomes more common, the bottleneck shifts from writing code to verifying code. A well-designed verification harness turns AI from an unaudited author into a supervised contributor whose output can be mechanically checked. **The harness is the trust boundary.**

---

## 6. Conclusion and Future Work

This project is a case study in AI-assisted systems programming with rigorous verification, not a production Redis replacement. It demonstrates that an AI agent can co-author a functional, reasonably performant Redis-compatible server -- 75+ commands, within 80--99% of Redis 7.4 throughput, passing the official test suite for implemented commands -- when paired with a human engineer who provides architectural direction and a verification harness that catches the AI's mistakes.

The verification methodology is the main contribution. Deterministic simulation testing, official compatibility suites, Jepsen-style linearizability checking, and controlled benchmarking together form a pipeline that makes AI-generated systems code auditable. The three bugs caught by DST -- empty collection cleanup, MSET postconditions, WATCH-inside-MULTI -- are the kinds of subtle correctness issues that pass code review and unit tests but fail under adversarial workloads.

Future work includes expanding Tcl suite coverage (bitmaps and blocking operations are the next frontier), adding persistence beyond the experimental S3 streaming layer, and exploring cluster-mode sharding across multiple machines. A particularly interesting direction is closing the loop between verification and production: connecting the server to observability systems like Datadog to feed real-world performance data, error rates, and latency distributions back into the development cycle. The project already includes optional Datadog integration (metrics, tracing, and logging via feature flag). The vision is a feedback loop where production telemetry informs which commands to optimize, which edge cases to harden, and which verification layers need strengthening -- turning observability into a fifth verification layer that operates on real traffic rather than synthetic workloads.

On the verification side, the open question is whether this methodology scales: as AI capabilities improve and the generated code grows more complex, do four verification layers remain sufficient, or does the harness itself need to evolve?

We do not yet have an answer, and we are skeptical of anyone who claims to.

---

**Source code:** https://github.com/nerdsane/redis-rust

**Verification harness:** See `HARNESS.md` in the repository for runnable commands and expected outputs.
