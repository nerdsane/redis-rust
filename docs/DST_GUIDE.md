# Deterministic Simulation Testing (DST) Guide

redis-rust implements Deterministic Simulation Testing inspired by FoundationDB and
TigerBeetle. This guide documents the real infrastructure as it exists in the codebase.
Every type name, function signature, and file path in this document is drawn directly
from the source.

> **Maintainer note:** When you change a DST harness, fault constant, or config preset,
> update this guide in the same commit. Every code block here must compile against the
> real crate.

---

## Quick Start for Agents

**Read this first.** This section tells you what to do based on the task you have
been given. Skip to the linked section for details.

### "I need to add a new Redis command"

After implementing the command in the executor, you MUST add DST coverage:

1. Open `src/redis/executor_dst.rs`.
2. Add a new `run_<category>_op` branch or extend an existing one (e.g.,
   `run_string_op`, `run_list_op`).
3. Update the `ShadowState` if the command mutates state -- add helper methods
   like `shadow.set_string()`, `shadow.del()`, etc.
4. Follow the extract-then-assert pattern to avoid borrow-checker conflicts (see
   [Borrow Checker Traps](#borrow-checker-traps-in-dst-harnesses)).
5. Run: `cargo test --lib executor_dst -- --nocapture`
6. If adding fields to the `Command::Set` variant or similar, update ALL ~25+
   struct literals across test files.
7. Update this guide if you added a new command category or config weight.

See [Executor DST](#executor-dst) for the full harness API.

### "I need to add a new fault type"

1. Add the constant to the appropriate module in `src/buggify/faults.rs`.
2. Add it to `ALL_FAULTS` in the same file.
3. Set its default probability in `FaultConfig::moderate()` (and optionally
   `calm()` and `chaos()`) in `src/buggify/config.rs`.
4. Use `buggify!(&mut rng, faults::your_module::YOUR_FAULT)` at the injection
   site in the simulated I/O layer (`src/io/simulation.rs`).
5. Update the fault catalog table in this guide.

See [Fault Injection](#fault-injection-buggify) for the full API.

### "I need to verify a CRDT property"

1. Add a Kani proof in `src/replication/lattice.rs` inside the
   `#[cfg(kani)] mod kani_proofs` block.
2. Add a DST harness test in `src/replication/crdt_dst.rs` using the existing
   `CRDTDSTConfig` pattern.
3. If the property is architectural (e.g., convergence under partition), add a
   multi-node simulation test in `src/simulator/multi_node.rs`.

See [Kani Bounded Proofs](#kani-bounded-proofs) and [CRDT DST](#crdt-dst).

### "I need to understand how a DST test works"

Read these sections in order:
1. [Why DST](#why-dst----the-core-insight) -- the mental model
2. [How Determinism Is Achieved](#how-determinism-is-achieved) -- the contract
3. [Executor DST](#executor-dst) -- the most complete example
4. [Sequence Diagrams](#sequence-diagrams) -- visual flow of each harness

### "Tests are failing with a borrow-checker error in a DST harness"

See [Borrow Checker Traps](#borrow-checker-traps-in-dst-harnesses). The short
answer: extract the expected value from `self.shadow.get()` into a local variable
(or local enum) BEFORE calling `self.assert_*()`.

### "I changed code and a DST seed that used to pass now fails"

That is expected. Seeds encode a path through the code; changing the code changes
the path. See [Seed Fragility](#seed-fragility) in the Limitations section.
Debug the failure, fix the bug, and the seed will pass again. Do NOT just delete
the test.

---

## Table of Contents

1. [Overview](#overview)
2. [Why DST -- The Core Insight](#why-dst----the-core-insight)
3. [How Determinism Is Achieved](#how-determinism-is-achieved)
4. [Fault Injection (buggify)](#fault-injection-buggify)
5. [Simulated I/O Layer](#simulated-io-layer)
6. [Executor DST](#executor-dst)
7. [Transaction DST](#transaction-dst)
8. [Connection-Level Transaction DST](#connection-level-transaction-dst)
9. [CRDT DST](#crdt-dst)
10. [Multi-Node Simulation](#multi-node-simulation)
11. [Zipfian Workload Generation](#zipfian-workload-generation)
12. [Stateright Model Checking](#stateright-model-checking)
13. [Kani Bounded Proofs](#kani-bounded-proofs)
14. [TLA+ Specifications](#tla-specifications)
15. [Maelstrom Integration](#maelstrom-integration)
16. [Running the Tests](#running-the-tests)
17. [Debugging Failed Seeds](#debugging-failed-seeds)
18. [Bugs Found by DST](#bugs-found-by-dst)
19. [Limitations and Trade-offs](#limitations-and-trade-offs)
20. [Borrow Checker Traps in DST Harnesses](#borrow-checker-traps-in-dst-harnesses)
21. [Recipe: Adding a New DST Harness](#recipe-adding-a-new-dst-harness)
22. [Sequence Diagrams](#sequence-diagrams)
23. [File Locations](#file-locations)

---

## Overview

DST finds bugs that would take years to manifest in production by combining:

- **Deterministic replay** -- same seed produces the same execution, making every
  failure reproducible.
- **Accelerated virtual time** -- hours of real-world scenarios run in seconds.
- **Fault injection** -- 35+ named faults across network, timer, process, disk,
  object-store, and replication categories.
- **Shadow-state oracle** -- a reference model runs in parallel with the real executor;
  every response is checked against the expected value.
- **Exhaustive exploration** -- Stateright model checking and Kani bounded proofs
  verify critical CRDT invariants.

```
                      Verification Pyramid

    Formal proofs          Simulation            Production
    (exhaustive)           (randomised)          (monitoring)
         |                      |                     |
         v                      v                     v
   Stateright/Kani       35+ fault types         Tcl test suite
   TLA+ specs            Shadow-state oracle     Benchmarks
```

---

## Why DST -- The Core Insight

Traditional testing uses fixed scenarios: "set key A, get key A, assert value." This
covers the happy path but misses the combinatorial explosion of interleavings,
failures, and timing that real systems face. As Will Wilson (FoundationDB) put it,
simulation testing found "all of the bugs in the database" -- with only one or two
customer-reported bugs in the company's history.

The insight is: **treat your production code as the simulation model.** Instead of
building a separate mathematical model, run the real `CommandExecutor`, real CRDT
merge logic, and real transaction state machine -- but replace everything
non-deterministic (time, randomness, network, disk) with controlled stubs.

Our DST harnesses follow the **FoundationDB simulation testing** methodology
(also adopted by TigerBeetle and Antithesis). Each harness:

1. Takes a **seed** that controls all randomness.
2. Generates a **random workload** (commands, fault injections, timing).
3. Maintains a **shadow state** (reference model) alongside the real system.
4. Checks invariants **after every operation**, not just at the end.
5. Reports the seed on failure so the exact execution can be replayed.

The shadow state is sometimes called an "oracle" in the testing literature. It is a
simple, obviously-correct implementation (e.g., a `HashMap`) that answers "what
*should* the system return?" After each command, the harness compares the real
executor's `RespValue` against the oracle's prediction. Any divergence is an
invariant violation.

---

## How Determinism Is Achieved

Determinism means: **same seed + same code = same execution.** This requires
eliminating every source of non-determinism.

### The I/O Abstraction Contract

All non-deterministic operations go through trait interfaces defined in
`src/io/mod.rs`:

| Trait | What it abstracts | Simulated impl |
|-------|-------------------|----------------|
| `Rng` | Random number generation | `SimulatedRng` (ChaCha8, seeded) |
| `Clock` | Wall-clock time | `SimulatedClock` (reads `SimulationContext`) |
| `TimeSource` | Millisecond timestamps | `SimulatedTimeSource` (virtual time + per-node offset) |
| `Network` | TCP bind/connect/read/write | `SimulatedNetwork` (in-memory, fault-injected) |
| `Runtime` | Task spawning | `SimulatedRuntime` |

In production, these traits are implemented by `ProductionRng`, real system clocks,
and Tokio-based networking. In simulation, every implementation reads from the
shared `SimulationContext`, which holds the single `SimulatedRng` instance. Because
ChaCha8 is a deterministic CSPRNG, the same seed always produces the same stream of
random decisions -- which command to run, whether to drop a packet, how long to
delay, which key to access.

### Single-Threaded Execution

DST requires single-threaded execution (or at minimum, deterministic scheduling of
concurrent tasks). Our executor DST harnesses run in a single thread -- the
`ExecutorDSTHarness` calls `executor.execute()` synchronously in a loop. The
multi-node simulation (`MultiNodeSimulation`) is also single-threaded: gossip
rounds and time advancement are driven explicitly by the test, not by real timers.

The connection-level transaction DST (`tests/connection_transaction_dst.rs`) uses
`#[tokio::test]` but runs two `SimulatedConnection` instances sequentially within
the same async task, avoiding OS-level scheduling non-determinism.

### What About Threads in Production?

The production server uses Tokio for async I/O. DST does not test the Tokio
scheduler's interleaving decisions -- that is a known limitation. We compensate by:
- Testing transaction conflict detection at the connection level (explicit interleaving)
- Using Stateright for exhaustive state-space exploration of CRDT merges
- Using Maelstrom for Jepsen-style black-box distributed testing

---

## Fault Injection (buggify)

Source files:
- `src/buggify/faults.rs` -- named fault constants
- `src/buggify/config.rs` -- `FaultConfig` with presets
- `src/buggify/mod.rs` -- `should_buggify()`, macros, stats

### Fault Catalog

Every fault has a dotted string identifier defined as a `&str` constant.

| Category | Constant | String value | Default (moderate) |
|----------|----------|-------------|-------------------|
| **Network** | `network::PACKET_DROP` | `"network.packet_drop"` | 1% |
| | `network::PACKET_CORRUPT` | `"network.packet_corrupt"` | 0.1% |
| | `network::PARTIAL_WRITE` | `"network.partial_write"` | 0.5% |
| | `network::REORDER` | `"network.reorder"` | 2% |
| | `network::CONNECTION_RESET` | `"network.connection_reset"` | 0.5% |
| | `network::CONNECT_TIMEOUT` | `"network.connect_timeout"` | 1% |
| | `network::DELAY` | `"network.delay"` | 5% |
| | `network::DUPLICATE` | `"network.duplicate"` | 0.5% |
| **Timer** | `timer::DRIFT_FAST` | `"timer.drift_fast"` | 1% |
| | `timer::DRIFT_SLOW` | `"timer.drift_slow"` | 1% |
| | `timer::SKIP` | `"timer.skip"` | 1% |
| | `timer::DUPLICATE` | `"timer.duplicate"` | 0.5% |
| | `timer::JUMP_FORWARD` | `"timer.jump_forward"` | 0.1% |
| | `timer::JUMP_BACKWARD` | `"timer.jump_backward"` | 0.05% |
| **Process** | `process::CRASH` | `"process.crash"` | 0.1% |
| | `process::PAUSE` | `"process.pause"` | 1% |
| | `process::SLOW` | `"process.slow"` | 2% |
| | `process::OOM` | `"process.oom"` | 0.01% |
| | `process::CPU_STARVATION` | `"process.cpu_starvation"` | 1% |
| **Disk** | `disk::WRITE_FAIL` | `"disk.write_fail"` | 0.1% |
| | `disk::PARTIAL_WRITE` | `"disk.partial_write"` | 0.1% |
| | `disk::CORRUPTION` | `"disk.corruption"` | 0.01% |
| | `disk::SLOW` | `"disk.slow"` | 2% |
| | `disk::FSYNC_FAIL` | `"disk.fsync_fail"` | 0.05% |
| | `disk::STALE_READ` | `"disk.stale_read"` | 0.1% |
| | `disk::DISK_FULL` | `"disk.disk_full"` | 0.01% |
| **Object Store** | `object_store::PUT_FAIL` | `"object_store.put_fail"` | -- |
| | `object_store::GET_FAIL` | `"object_store.get_fail"` | -- |
| | `object_store::GET_CORRUPT` | `"object_store.get_corrupt"` | -- |
| | `object_store::TIMEOUT` | `"object_store.timeout"` | -- |
| | `object_store::PARTIAL_WRITE` | `"object_store.partial_write"` | -- |
| | `object_store::DELETE_FAIL` | `"object_store.delete_fail"` | -- |
| | `object_store::LIST_INCOMPLETE` | `"object_store.list_incomplete"` | -- |
| | `object_store::RENAME_FAIL` | `"object_store.rename_fail"` | -- |
| | `object_store::SLOW` | `"object_store.slow"` | -- |
| **Replication** | `replication::GOSSIP_DROP` | `"replication.gossip_drop"` | 2% |
| | `replication::GOSSIP_DELAY` | `"replication.gossip_delay"` | 5% |
| | `replication::GOSSIP_CORRUPT` | `"replication.gossip_corrupt"` | 0.1% |
| | `replication::SPLIT_BRAIN` | `"replication.split_brain"` | 0.01% |
| | `replication::STALE_REPLICA` | `"replication.stale_replica"` | 1% |

The full list is also available at runtime via `buggify::ALL_FAULTS`.

### FaultConfig Presets

`FaultConfig` lives in `src/buggify/config.rs`. It stores per-fault probabilities in
a `HashMap<&'static str, f64>` and applies a `global_multiplier`.

```rust
use crate::buggify::FaultConfig;
use crate::buggify::faults;

// Presets
let _disabled = FaultConfig::disabled();        // enabled=false, multiplier=0.0
let _calm     = FaultConfig::calm();            // multiplier=0.1, few faults
let _moderate = FaultConfig::moderate();        // multiplier=1.0 (default)
let _chaos    = FaultConfig::chaos();           // multiplier=3.0, aggressive rates

// Custom configuration
let mut config = FaultConfig::new();            // empty, enabled=true
config.set(faults::network::PACKET_DROP, 0.05); // 5%
config.set(faults::network::DELAY, 0.10);       // 10%
config.global_multiplier = 2.0;                 // doubles effective probabilities

// Builder pattern
let config = FaultConfig::new()
    .with_network_faults()   // sets PACKET_DROP, PACKET_CORRUPT, REORDER, DELAY
    .with_timer_faults()     // sets DRIFT_FAST, DRIFT_SLOW, SKIP
    .with_process_faults()   // sets CRASH, PAUSE, SLOW
    .with_multiplier(2.0);

// Query effective probability (base * global_multiplier, clamped to [0,1])
let prob = config.get(faults::network::PACKET_DROP);

// Check whether a fault should trigger given a random value
let triggered = config.should_trigger(faults::network::PACKET_DROP, 0.005);
```

### Using buggify in Code

The `should_buggify` function and its macro wrappers take a `&mut impl io::Rng`
for deterministic randomness.

```rust
use crate::buggify::{self, faults};
use crate::io::simulation::SimulatedRng;

let mut rng = SimulatedRng::new(42);

// Named fault -- uses probability from the thread-local FaultConfig
if buggify!(&mut rng, faults::network::PACKET_DROP) {
    return; // drop the packet
}

// Explicit probability override
if buggify!(&mut rng, faults::network::DELAY, 0.10) {
    // inject delay
}

// Location-based (auto-generates fault ID from file:line)
if buggify_here!(&mut rng) { /* ... */ }

// Convenience macros with fixed probabilities
if buggify_rarely!(&mut rng, "my.fault")    { /* 0.1%  */ }
if buggify_sometimes!(&mut rng, "my.fault") { /* 5%    */ }
if buggify_often!(&mut rng, "my.fault")     { /* 20%   */ }

// Suppress all faults in a critical section
{
    let _guard = suppress_buggify!();
    // no faults will fire here
}
```

Stats tracking is built in:

```rust
buggify::reset_stats();
// ... run test ...
let stats = buggify::get_stats();
println!("{}", stats.summary());
// Output: "network.packet_drop: 12/1000 (1.20%)"
```

---

## Simulated I/O Layer

Source files:
- `src/io/mod.rs` -- `Rng` trait, `Timestamp`, `Clock`, `Network`, `Runtime` traits
- `src/io/simulation.rs` -- `SimulatedRng`, `SimulatedClock`, `SimulatedNetwork`,
  `SimulationContext`, `SimulatedTimeSource`, `ClockOffset`
- `src/simulator/time.rs` -- `VirtualTime`, `Duration`
- `src/simulator/rng.rs` -- `DeterministicRng` (ChaCha8-based)

### SimulatedRng

Wraps `rand_chacha::ChaCha8Rng` and implements the `io::Rng` trait.
Same seed always produces the same sequence.

```rust
use crate::io::simulation::SimulatedRng;
use crate::io::Rng; // trait

let mut rng = SimulatedRng::new(42);

let val = rng.next_u64();
let coin = rng.gen_bool(0.5);
let idx = rng.gen_range(0, 100);  // [0, 100)
rng.shuffle(&mut my_slice);
```

### VirtualTime and Duration

Defined in `src/simulator/time.rs`. Both wrap a `u64` representing milliseconds.

```rust
use crate::simulator::time::{VirtualTime, Duration};

let t = VirtualTime::from_millis(5000);
let t2 = VirtualTime::from_secs(10);
assert_eq!(t2.as_millis(), 10_000);

let d = Duration::from_millis(100);
let later = t + d;          // VirtualTime + Duration -> VirtualTime
let elapsed = t2 - t;       // VirtualTime - VirtualTime -> Duration
```

### SimulationContext

`SimulationContext` (`src/io/simulation.rs`) is the shared kernel for a simulation
run. It owns global time, per-node clock offsets, the RNG, and the fault config.

```rust
use crate::io::simulation::{SimulationContext, NodeId, ClockOffset};
use crate::io::Timestamp;
use crate::buggify::FaultConfig;
use std::sync::Arc;

let ctx = Arc::new(SimulationContext::new(/*seed=*/42, FaultConfig::moderate()));

// Time control
assert_eq!(ctx.now().as_millis(), 0);
ctx.advance_by(Duration::from_millis(100));
ctx.advance_to(Timestamp::from_millis(500));

// Per-node clock skew
ctx.set_clock_offset(NodeId(1), ClockOffset {
    fixed_offset_ms: 50,      // 50ms ahead
    drift_ppm: 1000,           // 0.1% faster
    drift_anchor: Timestamp::ZERO,
});
let local = ctx.local_time(NodeId(1)); // applies offset + drift
```

### SimulatedTimeSource

Implements the `TimeSource` trait, reading from a `SimulationContext` with
per-node clock offset applied.

```rust
use crate::io::simulation::SimulatedTimeSource;

let ts = SimulatedTimeSource::new(ctx.clone(), NodeId(0));
let ms = ts.now_millis(); // reads ctx.local_time(NodeId(0))
```

---

## Executor DST

Source: `src/redis/executor_dst.rs`

The `ExecutorDSTHarness` is a shadow-state harness that exercises every command type
through `CommandExecutor::execute()`. It maintains a **shadow state** (reference
model) and checks invariants after every operation.

### ExecutorDSTConfig

```rust
use crate::redis::executor_dst::{ExecutorDSTConfig, ExecutorDSTHarness};

// Default: 50 keys, 30 values, 20 fields, zipf_exponent=1.0
let config = ExecutorDSTConfig::new(/*seed=*/42);

// Presets
let calm  = ExecutorDSTConfig::calm(42);           // 20 keys, 10 values, 10 fields
let chaos = ExecutorDSTConfig::chaos(42);          // 100 keys, 50 values, 30 fields, zipf=1.5
let heavy = ExecutorDSTConfig::string_heavy(42);   // weight_string=60

// Command category weights (default sum = 100)
// weight_string=30, weight_key=10, weight_list=15, weight_set=10,
// weight_hash=15, weight_sorted_set=10, weight_expiry=10
```

### Creating and Running

```rust
// Simplest form
let mut harness = ExecutorDSTHarness::with_seed(42);
harness.run(500); // execute 500 random operations
assert!(harness.result().is_success(), "Seed 42 failed");

// With explicit config
let config = ExecutorDSTConfig::chaos(99);
let mut harness = ExecutorDSTHarness::new(config);
harness.run(500);
let result = harness.result();
println!("{}", result.summary());
// "Seed 99: 500 ops (str:152, key:48, list:75, set:52, hash:73, zset:50, exp:50), 0 violations"

for v in &result.invariant_violations {
    println!("  VIOLATION: {}", v);
}
assert!(result.is_success());
```

### Batch Runner

```rust
use crate::redis::executor_dst::{run_executor_batch, summarize_executor_batch};

let results = run_executor_batch(
    /*start_seed=*/0,
    /*num_seeds=*/10,
    /*ops_per_seed=*/500,
    ExecutorDSTConfig::new,   // config factory fn(u64) -> ExecutorDSTConfig
);
println!("{}", summarize_executor_batch(&results));

let passed = results.iter().filter(|r| r.is_success()).count();
assert_eq!(passed, 10);
```

### What It Tests

The harness covers: SET, GET, SETNX, MSET/MGET, INCR, INCRBY, DECRBY, DECR,
INCRBYFLOAT, APPEND, STRLEN, GETRANGE, SETRANGE, GETDEL, GETSET,
DEL, EXISTS, TYPE, RENAME, RENAMENX, LPUSH, RPUSH, LPOP, RPOP, LLEN, LRANGE,
LINDEX, SADD, SREM, SMEMBERS, SISMEMBER, SCARD, SPOP,
HSET, HGET, HDEL, HEXISTS, HLEN, HGETALL, HKEYS, HVALS,
ZADD, ZSCORE, ZRANK, ZCARD, ZRANGE, ZREM,
EXPIRE, TTL, PERSIST, PING, ECHO, SELECT, CONFIG SET/GET/RESETSTAT.

Key access follows a Zipfian-like distribution via `zipfian_index()` so that a
small set of "hot keys" receives most of the traffic, matching real workloads.

---

## Transaction DST

Source: `src/redis/transaction_dst.rs`

The `TransactionDSTHarness` tests MULTI/EXEC/WATCH/DISCARD semantics at the
executor level. Two simulated clients share a single `CommandExecutor` and
interleave commands to test optimistic-locking conflict detection.

### TransactionDSTConfig

```rust
use crate::redis::transaction_dst::{TransactionDSTConfig, TransactionDSTHarness};

let config = TransactionDSTConfig::new(42);           // 20 keys, conflict=0.3
let hc = TransactionDSTConfig::high_conflict(42);     // 5 keys, conflict=0.6
let eh = TransactionDSTConfig::error_heavy(42);       // error_prob=0.3, discard=0.2
```

### Creating and Running

```rust
let mut harness = TransactionDSTHarness::with_seed(42);
harness.run(200);
let result = harness.result();
println!("{}", result.summary());
assert!(result.is_success());
```

### Scenarios Covered

| Scenario | Method | Invariant |
|----------|--------|-----------|
| WATCH + no mutation -> EXEC succeeds | `run_watch_no_conflict_scenario` | Watched key unchanged means transaction commits |
| WATCH + mutation -> EXEC returns nil | `run_watch_conflict_scenario` | External write invalidates the transaction |
| MULTI/EXEC without WATCH | `run_simple_exec_scenario` | Atomic execution of queued commands |
| MULTI then DISCARD | `run_discard_scenario` | Queue is cleared, state unchanged |
| Nested MULTI / EXEC without MULTI | `run_error_scenario` | Correct error messages returned |
| UNWATCH then EXEC | `run_unwatch_scenario` | UNWATCH clears watch state |

---

## Connection-Level Transaction DST

Source: `tests/connection_transaction_dst.rs`

This integration test exercises the transaction state machine as implemented in the
production connection handler (`connection_optimized.rs`), using
`ShardedActorState` as the backend. Two `SimulatedConnection` instances share the
same `ShardedActorState`, testing cross-connection WATCH conflicts.

```rust
// The async test harness (cannot be called directly, only via #[tokio::test])
async fn run_connection_transaction_dst(seed: u64) -> Vec<String> { /* ... */ }
```

The test function creates two connections, seeds initial keys, then runs 200
iterations choosing randomly among:
- WATCH + no conflict (EXEC succeeds)
- WATCH + conflict from conn_b (EXEC returns nil)
- Simple MULTI/EXEC
- DISCARD
- Error scenarios (nested MULTI, EXEC without MULTI, WATCH inside MULTI)

```bash
cargo test --test connection_transaction_dst
```

---

## CRDT DST

Source: `src/replication/crdt_dst.rs`

Four shadow-state DST harnesses verify CRDT convergence for:

| Harness | CRDT type | Operations |
|---------|-----------|------------|
| `GCounterDSTHarness` | `GCounter` | `increment_by(replica_id, amount)` |
| `PNCounterDSTHarness` | `PNCounter` | `increment_by` / `decrement_by` |
| `ORSetDSTHarness` | `ORSet<String>` | `add(elem, replica_id)` / `remove(&elem)` |
| `VectorClockDSTHarness` | `VectorClock` | `increment(replica_id)` |

### CRDTDSTConfig

```rust
use crate::replication::crdt_dst::{CRDTDSTConfig, GCounterDSTHarness};

let calm     = CRDTDSTConfig::calm(42);     // 3 replicas, no drops
let moderate = CRDTDSTConfig::moderate(42);  // 5 replicas, 10% drop, 5% partition
let chaos    = CRDTDSTConfig::chaos(42);     // 7 replicas, 30% drop, 15% partition
```

### Protocol: run -> sync_all -> check_convergence

```rust
let config = CRDTDSTConfig::calm(42);
let mut harness = GCounterDSTHarness::new(config);

harness.run(100);              // random operations across replicas
harness.sync_all();            // pairwise merge, multiple rounds
harness.check_convergence();   // all replicas must agree

let result = harness.result();
assert!(result.is_success());  // is_success = no violations AND converged
println!("{}", result.summary());
// "Seed 42: 100 ops, 15 syncs, 0 drops, converged=true, 0 violations"
```

`sync_all()` performs 5 rounds of pairwise merges. Messages can be dropped based
on `message_drop_prob`, simulating unreliable gossip. After enough rounds the
CRDTs must still converge.

### Determinism Verification

```rust
let mut h1 = GCounterDSTHarness::new(CRDTDSTConfig::calm(12345));
h1.run(50);
let mut h2 = GCounterDSTHarness::new(CRDTDSTConfig::calm(12345));
h2.run(50);
assert_eq!(h1.result().total_operations, h2.result().total_operations);
```

---

## Multi-Node Simulation

Source: `src/simulator/multi_node.rs`

`MultiNodeSimulation` provides a full cluster harness with `SimulatedNode`
instances, a `DeterministicRng`, network partitions, packet loss, message delay,
gossip rounds, anti-entropy sync via Merkle trees, and an operation history for
linearizability checking.

### Creating a Simulation

```rust
use crate::simulator::multi_node::MultiNodeSimulation;
use crate::redis::{Command, SDS};

// Basic 3-node cluster
let mut sim = MultiNodeSimulation::new(/*num_nodes=*/3, /*seed=*/42);

// With packet loss
let mut sim = MultiNodeSimulation::new(3, 42)
    .with_packet_loss(0.3)
    .with_message_delay(/*min_ms=*/1, /*max_ms=*/50);

// Without automatic anti-entropy on partition heal
let mut sim = MultiNodeSimulation::new_without_anti_entropy(3, 42);

// Partitioned mode with hash ring and selective gossip routing
let mut sim = MultiNodeSimulation::new_partitioned(
    /*num_nodes=*/10,
    /*replication_factor=*/3,
    /*seed=*/42,
);
```

### Operations

```rust
// Execute a command on node 0 as client 1
let resp = sim.execute(/*client_id=*/1, /*node_id=*/0,
    Command::set("key1".into(), SDS::from_str("value1")));

// Network partitions
sim.partition(0, 2);       // nodes 0 and 2 cannot communicate
sim.heal_partition(0, 2);  // heals partition, triggers anti-entropy if enabled

// Time advancement
sim.advance_time_ms(100);

// Gossip propagation
sim.gossip_round();

// Anti-entropy
sim.run_anti_entropy_sync(/*node_a=*/0, /*node_b=*/1);
sim.run_full_anti_entropy(); // all connected pairs

// Check communication
assert!(sim.can_communicate(0, 1));
```

### TimestampedOperation

Every `execute()` call records a `TimestampedOperation` in `sim.history`,
capturing `client_id`, `node_id`, `invoke_time`, `complete_time`, `command`, and
`response`. This enables post-hoc linearizability checking.

---

## Zipfian Workload Generation

Source: `src/simulator/dst_integration.rs`

`ZipfianGenerator` produces realistic hot/cold key distributions following Zipf's
law. With `skew=1.0` and 1000 keys, the top ~10 keys receive about 50% of
accesses.

```rust
use crate::simulator::dst_integration::{ZipfianGenerator, KeyDistribution};
use crate::io::simulation::SimulatedRng;

let zipf = ZipfianGenerator::new(/*num_keys=*/1000, /*skew=*/1.0);
let mut rng = SimulatedRng::new(42);

let key_index = zipf.sample(&mut rng);       // u64 in [0, 1000)
let key_string = zipf.generate_key(&mut rng); // "key42"
```

The `KeyDistribution` enum selects between uniform and Zipfian:

```rust
// ZipfianGenerator from src/simulator/dst_integration.rs
let zipf = ZipfianGenerator::new(1000, 1.0); // num_keys, skew
let hot_key_index = zipf.sample(&mut rng);    // returns 0..num_keys
```

The executor DST harness uses its own Zipfian-like approximation via
`zipfian_index()` (power-law on the unit interval), configured by
`ExecutorDSTConfig::zipf_exponent`.

---

## Stateright Model Checking

Source: `src/stateright/replication.rs`

The `CrdtMergeModel` exhaustively explores a state space of CRDT operations
(Set, Delete, Sync) over 2 replicas, 1 key, 2 values, and clock bound 3.

### Types

```rust
// Simplified LWW register for model checking
pub struct LwwRegister {
    pub value: Option<u64>,
    pub timestamp: u64,
    pub replica_id: ReplicaId,
    pub tombstone: bool,
}

// Actions
pub enum CrdtAction {
    Set { replica: ReplicaId, key: u64, value: u64 },
    Delete { replica: ReplicaId, key: u64 },
    Sync { from: ReplicaId, to: ReplicaId, key: u64 },
}

// State
pub struct CrdtState {
    pub replicas: BTreeMap<ReplicaId, BTreeMap<u64, LwwRegister>>,
    pub clocks: BTreeMap<ReplicaId, u64>,
}
```

### Properties Verified

The model checks three invariants:

1. **`lamport_monotonic`** -- clocks never exceed `max_clock`.
2. **`tombstone_consistency`** -- tombstone implies `value.is_none()`.
3. **`valid_timestamps`** -- no register timestamp exceeds `max_clock + 1`.

Additionally, standalone functions verify the algebraic merge laws:

```rust
verify_merge_commutative(a, b)   // merge(a,b) == merge(b,a)
verify_merge_associative(a, b, c) // merge(a, merge(b,c)) == merge(merge(a,b), c)
verify_merge_idempotent(a)       // merge(a,a) == a
```

### Running

```bash
# Exhaustive model check (ignored by default, ~minutes)
cargo test --lib stateright_replication_model_check -- --ignored --nocapture
```

---

## Kani Bounded Proofs

Source: `src/replication/lattice.rs` (inside `#[cfg(kani)] mod kani_proofs`)

Seven Kani proofs verify core CRDT properties with fully symbolic inputs:

| Proof | Property |
|-------|----------|
| `verify_lww_merge_commutative` | `LwwRegister::merge` is commutative |
| `verify_lww_merge_idempotent` | `LwwRegister::merge` is idempotent |
| `verify_lamport_clock_total_order` | `LamportClock` ordering is total |
| `verify_gcounter_merge_commutative` | `GCounter::merge` is commutative |
| `verify_gcounter_merge_idempotent` | `GCounter::merge` is idempotent |
| `verify_pncounter_merge_commutative` | `PNCounter::merge` is commutative |
| `verify_vector_clock_merge_commutative` | `VectorClock::merge` is commutative |

Example from the source (actual code):

```rust
#[kani::proof]
#[kani::unwind(5)]
fn verify_lww_merge_commutative() {
    let r1 = ReplicaId::new(kani::any());
    let r2 = ReplicaId::new(kani::any());

    let a: LwwRegister<u64> = LwwRegister {
        value: kani::any(),
        timestamp: LamportClock {
            time: kani::any(),
            replica_id: r1,
        },
        tombstone: kani::any(),
    };

    let b: LwwRegister<u64> = LwwRegister {
        value: kani::any(),
        timestamp: LamportClock {
            time: kani::any(),
            replica_id: r2,
        },
        tombstone: kani::any(),
    };

    let ab = a.merge(&b);
    let ba = b.merge(&a);
    kani::assert(ab == ba, "LWW merge must be commutative");
}
```

### Running

```bash
cargo kani --harness verify_lww_merge_commutative
cargo kani --harness verify_gcounter_merge_commutative
cargo kani --harness verify_pncounter_merge_commutative
cargo kani --harness verify_vector_clock_merge_commutative
cargo kani --harness verify_lamport_clock_total_order
```

---

## TLA+ Specifications

Four TLA+ specs define the formal models. They live in `specs/tla/`:

| Specification | File | What it models |
|---------------|------|----------------|
| Replication Convergence | `specs/tla/ReplicationConvergence.tla` | CRDT merge properties |
| Gossip Protocol | `specs/tla/GossipProtocol.tla` | Gossip delivery guarantees |
| Streaming Persistence | `specs/tla/StreamingPersistence.tla` | Durability and ordering |
| Anti-Entropy | `specs/tla/AntiEntropy.tla` | Merkle tree reconciliation |

---

## Maelstrom Integration

Two Maelstrom binaries provide Jepsen-style distributed testing:

- `src/bin/maelstrom_kv.rs` -- single-node KV store
- `src/bin/maelstrom_kv_replicated.rs` -- replicated KV store

```bash
# Build
cargo build --release --bin maelstrom-kv-replicated

# Run linearizability test (requires Java 11+ and Maelstrom installed)
./maelstrom/maelstrom test -w lin-kv \
    --bin ./target/release/maelstrom-kv-replicated \
    --node-count 3 \
    --time-limit 60 \
    --rate 100 \
    --concurrency 10

# With nemesis (network partition injection)
./maelstrom/maelstrom test -w lin-kv \
    --bin ./target/release/maelstrom-kv-replicated \
    --node-count 5 \
    --time-limit 120 \
    --rate 50 \
    --nemesis partition
```

---

## Running the Tests

### Executor DST

```bash
# Single seed
cargo test --lib executor_dst -- test_executor_dst_single_seed --nocapture

# Calm / Chaos / String-heavy presets
cargo test --lib executor_dst -- test_executor_dst_calm --nocapture
cargo test --lib executor_dst -- test_executor_dst_chaos --nocapture
cargo test --lib executor_dst -- test_executor_dst_string_heavy --nocapture

# 10 seeds batch
cargo test --lib executor_dst -- test_executor_dst_10_seeds --nocapture
```

### Transaction DST

```bash
cargo test --lib transaction_dst -- test_transaction_dst_single_seed --nocapture
cargo test --lib transaction_dst -- test_transaction_dst_high_conflict --nocapture
cargo test --lib transaction_dst -- test_transaction_dst_error_heavy --nocapture
cargo test --lib transaction_dst -- test_transaction_dst_10_seeds --nocapture
```

### Connection-Level Transaction DST

```bash
cargo test --test connection_transaction_dst -- --nocapture

# Specific tests
cargo test --test connection_transaction_dst test_connection_transaction_dst_single
cargo test --test connection_transaction_dst test_connection_transaction_dst_10_seeds
cargo test --test connection_transaction_dst test_connection_transaction_dst_100_seeds
```

### CRDT DST

```bash
# GCounter
cargo test --lib crdt_dst -- test_gcounter_dst_single_calm --nocapture
cargo test --lib crdt_dst -- test_gcounter_dst_100_seeds --nocapture
cargo test --lib crdt_dst -- test_gcounter_dst_moderate_100_seeds --nocapture

# PNCounter
cargo test --lib crdt_dst -- test_pncounter_dst --nocapture

# ORSet
cargo test --lib crdt_dst -- test_orset_dst --nocapture

# VectorClock
cargo test --lib crdt_dst -- test_vectorclock_dst --nocapture

# Determinism check
cargo test --lib crdt_dst -- test_crdt_dst_determinism --nocapture
```

### Multi-Node Simulation

```bash
cargo test --lib multi_node -- test_basic_replication --nocapture
cargo test --lib multi_node -- test_partition_and_heal --nocapture
cargo test --lib multi_node -- test_concurrent_writes_converge --nocapture
cargo test --lib multi_node -- test_packet_loss_eventual_convergence --nocapture
cargo test --lib multi_node -- test_selective_gossip_message_reduction --nocapture
cargo test --lib multi_node -- test_multi_seed_convergence --nocapture
```

### Stateright

```bash
cargo test --lib stateright_replication_model_check -- --ignored --nocapture
```

### All DST Tests at Once

```bash
cargo test --lib dst --release -- --nocapture
```

---

## Debugging Failed Seeds

Every harness prints the seed and violation details on failure:

```
Seed 99: 437 ops (str:131, key:44, ...), 1 violations
  VIOLATION: GET key:3 should match shadow: expected BulkString([118, 97, ...]) got Null
```

Reproduce:

```rust
// In a test or main:
let mut harness = ExecutorDSTHarness::with_seed(99);
harness.run(500);
for v in &harness.result().invariant_violations {
    eprintln!("VIOLATION: {}", v);
}
```

The `last_op` field on `ExecutorDSTResult` / `TransactionDSTResult` records
exactly which operation triggered the first violation.

---

## Bugs Found by DST

DST has found real correctness bugs in this codebase that would have been extremely
difficult to catch with traditional unit tests.

### 1. Empty Collection Cleanup (FIXED)

LPOP, RPOP, SREM, SPOP, HDEL, and ZREM did not delete the key when a collection
became empty. In Redis, popping the last element from a list (or removing the last
member from a set) must delete the key entirely so that `EXISTS` returns 0 and
`TYPE` returns "none." The executor DST's shadow state tracked collection sizes and
caught the divergence: the shadow deleted the key when it became empty, but the real
executor left a zombie key behind.

### 2. MSET debug_assert Postcondition (FIXED)

The MSET implementation had a debug_assert postcondition that checked all key-value
pairs after the operation. When the same key appeared multiple times in the MSET
argument list, the postcondition checked each occurrence -- but Redis semantics say
the *last* value for a duplicate key wins. The shadow state in the executor DST
correctly applied last-value-wins, exposing the assert failure.

### 3. WATCH Inside MULTI (FIXED)

Issuing WATCH while already inside a MULTI block was getting queued (as if it were
a normal command) instead of returning an immediate error. The transaction DST
harness's error scenario runner tested exactly this case and caught the wrong
`RespValue` variant.

### Why DST Found These

All three bugs share a pattern: they involve **interactions between commands** or
**state transitions at boundaries** (empty collection, duplicate keys, nested state
machines). Unit tests that exercise one command in isolation miss these. DST's
random workload naturally explores these corner cases because it generates thousands
of command sequences with realistic key reuse (Zipfian distribution) and all command
types interleaved.

---

## Limitations and Trade-offs

DST is powerful but not a silver bullet. Understanding its boundaries helps you
decide when to use DST vs. other testing approaches.

### What DST Does NOT Test

- **OS-level behavior**: Real TCP, real disk I/O, real memory allocation. Our
  `SimulatedNetwork` drops packets in-memory; it does not test kernel buffer
  management or TCP congestion control.
- **Thread scheduling**: The production Tokio runtime makes scheduling decisions
  that DST does not explore. A race condition that depends on Tokio's specific
  task ordering will not be found by DST.
- **Performance**: DST verifies correctness, not throughput or latency. Use the
  `docker-benchmark/` suite for performance testing.
- **External integrations**: Anything outside the `io::` trait boundary (e.g., a
  real Redis client, a real Kafka consumer) is not simulated.

### Seed Fragility

A seed encodes a specific path through the code. When you change the code, the same
seed may take a different path. This means:

- A seed that found a bug last week may not reproduce it after a refactoring.
- Seeds are useful for **immediate debugging**, not long-term regression. Convert
  failing seeds into deterministic regression tests with fixed inputs.
- As Phil Eaton notes: "Seeds are useful only for converting into integration tests
  that survive refactoring."

### Workload Quality Matters

As Will Wilson (FoundationDB) observed: "Tuning all the random distributions, the
parameters of your system, the workload, the fault injection... is very challenging
and very labor intensive." Our `ExecutorDSTConfig` weight system and Zipfian
distribution are designed to create realistic workloads, but coverage is not
guaranteed. Branch coverage metrics are a poor proxy for DST effectiveness -- a
test can achieve 90% branch coverage while missing the one interleaving that
triggers a bug.

### Best Practices

1. **Run many seeds** -- 10 seeds is a smoke test; 100+ seeds per CI run is better.
   FoundationDB ran 5-10M simulation hours per night.
2. **Use Zipfian distributions** -- uniform key distribution is unrealistic and
   misses hot-key contention bugs.
3. **Check invariants after every operation** -- do not batch-check at the end.
   Early detection pinpoints the failing operation.
4. **Save and convert failing seeds** -- when a seed finds a bug, extract the
   minimal reproducing command sequence and add it as a permanent test.
5. **Combine fault presets** -- test with `calm`, `moderate`, and `chaos` to cover
   both clean-path and failure-path logic.
6. **Layer multiple failure modes** -- real failures are correlated (a disk slowdown
   often accompanies a GC pause). The `chaos` preset enables many fault types
   simultaneously.

---

## Borrow Checker Traps in DST Harnesses

This is the single most common source of compilation errors when modifying DST
harnesses. You MUST understand this pattern before editing `executor_dst.rs` or
`transaction_dst.rs`.

### The Problem

The harness struct holds both the shadow state and the assertion methods:

```rust
pub struct ExecutorDSTHarness {
    shadow: ShadowState,      // immutable borrow via shadow.get()
    result: ExecutorDSTResult, // mutable borrow via self.assert_*() -> self.violation()
    // ...
}
```

This code WILL NOT COMPILE:

```rust
// BAD: immutable borrow of self.shadow is alive when self.assert_*() needs &mut self
if let Some(RefValue::String(v)) = self.shadow.get(&key) {
    self.assert_bulk_eq(&resp, v, "GET should match shadow");
    //                          ^ borrows self mutably, but self.shadow already borrowed
}
```

### The Fix: Extract Into a Local

Copy the expected value out of the shadow state BEFORE calling any assertion method:

```rust
// GOOD: extract expected value into a local enum, then assert separately
enum GetExpect {
    Value(Vec<u8>),
    Null,
    WrongType,
}
let expect = match self.shadow.get(&key) {
    Some(RefValue::String(v)) => GetExpect::Value(v.clone()),
    None => GetExpect::Null,
    Some(_) => GetExpect::WrongType,
};
// Now shadow borrow is dropped -- safe to call self.assert_*()
match expect {
    GetExpect::Value(v) => {
        self.assert_bulk_eq(&resp, &v, &format!("GET {} should match shadow", key));
    }
    GetExpect::Null => {
        self.assert_null(&resp, &format!("GET {} non-existent should be nil", key));
    }
    GetExpect::WrongType => {
        self.assert_error_contains(&resp, "WRONGTYPE", &format!("GET {} on wrong type", key));
    }
}
```

This pattern appears throughout `executor_dst.rs` -- look for `enum IncrExpect`,
`enum GetExpect`, `enum GDExpect`, `enum GSExpect`, etc. Every command that reads
from the shadow and then asserts uses this pattern. Follow it exactly.

### When Adding a New Command to the Harness

1. Generate the command and execute it: `let resp = self.executor.execute(&cmd);`
2. Read expected state from shadow into a LOCAL variable or enum.
3. Drop the shadow borrow (the `match` or `let` binding ends the borrow).
4. Call `self.assert_*()` methods using the local variable.
5. Update the shadow state: `self.shadow.set_string(&key, value);`

---

## Recipe: Adding a New DST Harness

Follow this when you need to build a completely new DST harness for a subsystem
that does not yet have one (e.g., a new persistence layer, a new protocol).

### Step 1: Define Config

```rust
#[derive(Debug, Clone)]
pub struct MySubsystemDSTConfig {
    pub seed: u64,
    // ... parameters that control the workload shape
}

impl MySubsystemDSTConfig {
    pub fn new(seed: u64) -> Self { /* defaults */ }
    pub fn calm(seed: u64) -> Self { /* small state space */ }
    pub fn chaos(seed: u64) -> Self { /* large state space, high contention */ }
}
```

### Step 2: Define Result

```rust
#[derive(Debug, Clone)]
pub struct MySubsystemDSTResult {
    pub seed: u64,
    pub total_operations: u64,
    pub invariant_violations: Vec<String>,
    // ... per-category counters
}

impl MySubsystemDSTResult {
    pub fn is_success(&self) -> bool { self.invariant_violations.is_empty() }
    pub fn summary(&self) -> String { /* one-line summary */ }
}
```

### Step 3: Define the Harness

```rust
pub struct MySubsystemDSTHarness {
    config: MySubsystemDSTConfig,
    rng: SimulatedRng,           // from crate::io::simulation
    real_system: MyRealSystem,   // the thing under test
    shadow: MyShadowState,       // the oracle
    result: MySubsystemDSTResult,
}

impl MySubsystemDSTHarness {
    pub fn new(config: MySubsystemDSTConfig) -> Self { /* ... */ }
    pub fn with_seed(seed: u64) -> Self { Self::new(MySubsystemDSTConfig::new(seed)) }

    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            self.result.total_operations += 1;
            self.run_single_op();
            if !self.result.invariant_violations.is_empty() {
                break; // stop on first violation for easier debugging
            }
        }
    }

    pub fn result(&self) -> &MySubsystemDSTResult { &self.result }

    fn run_single_op(&mut self) { /* weighted random command selection */ }

    fn violation(&mut self, msg: &str) {
        self.result.invariant_violations.push(msg.to_string());
    }
}
```

### Step 4: Write Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_my_subsystem_dst_single_seed() {
        let mut harness = MySubsystemDSTHarness::with_seed(12345);
        harness.run(100);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success(), "Seed 12345 failed");
    }

    #[test]
    fn test_my_subsystem_dst_10_seeds() {
        for seed in 0..10 {
            let mut harness = MySubsystemDSTHarness::with_seed(seed);
            harness.run(500);
            assert!(harness.result().is_success(), "Seed {} failed", seed);
        }
    }
}
```

### Step 5: Add Batch Runner (Optional)

Follow the pattern from `run_executor_batch` in `executor_dst.rs`:

```rust
pub fn run_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> MySubsystemDSTConfig,
) -> Vec<MySubsystemDSTResult> { /* ... */ }
```

### Step 6: Update This Guide

Add a section documenting your new harness. Include: source file, config presets,
what it tests, and how to run it.

---

## Sequence Diagrams

### Executor DST: Single Operation Cycle

```
  Test Loop          ExecutorDSTHarness       ShadowState       CommandExecutor
     |                       |                     |                   |
     |--- run(N) ----------->|                     |                   |
     |                       |                     |                   |
     |                       |-- evict_expired --->|                   |
     |                       |<-- (expired keys) --|                   |
     |                       |                     |                   |
     |                       |-- select_category ->|                   |
     |                       |   (weighted random) |                   |
     |                       |                     |                   |
     |                       |-- random_key ------>|                   |
     |                       |   (zipfian dist.)   |                   |
     |                       |                     |                   |
     |                       |-- executor.execute(cmd) ------------->  |
     |                       |<-- RespValue -------------------------  |
     |                       |                     |                   |
     |                       |-- shadow.get(key) ->|                   |
     |                       |<-- RefValue --------|                   |
     |                       |                     |                   |
     |                       |-- assert_*(resp, expected)              |
     |                       |   (record violation if mismatch)        |
     |                       |                     |                   |
     |                       |-- shadow.set_*(key) |                   |
     |                       |                     |                   |
     |<-- result() ---------|                     |                   |
```

### Transaction DST: WATCH Conflict Scenario

```
  TransactionDSTHarness       CommandExecutor
          |                         |
          |-- SET key=value ------->|          (setup)
          |<-- OK ------------------|
          |                         |
          |-- WATCH [key] --------->|          (Client A watches)
          |<-- OK ------------------|
          |                         |
          |-- SET key=conflict ---->|          (Client B mutates)
          |<-- OK ------------------|
          |                         |
          |-- MULTI --------------->|          (Client A begins tx)
          |<-- OK ------------------|
          |                         |
          |-- SET key=new_value --->|          (queued, not executed)
          |<-- QUEUED --------------|
          |                         |
          |-- EXEC ---------------->|          (conflict detected!)
          |<-- BulkString(None) ----|          (nil = aborted)
          |                         |
          |-- GET key ------------->|          (verify conflict value persists)
          |<-- "conflict" ----------|
```

### Transaction DST: WATCH No-Conflict Scenario

```
  TransactionDSTHarness       CommandExecutor
          |                         |
          |-- SET key=value ------->|          (setup)
          |<-- OK ------------------|
          |                         |
          |-- WATCH [key] --------->|          (Client A watches)
          |<-- OK ------------------|
          |                         |
          |   (no external mutation)            |
          |                         |
          |-- MULTI --------------->|
          |<-- OK ------------------|
          |                         |
          |-- SET key=new_value --->|          (queued)
          |<-- QUEUED --------------|
          |                         |
          |-- EXEC ---------------->|          (no conflict)
          |<-- Array([OK]) ---------|          (committed!)
          |                         |
          |-- GET key ------------->|          (verify new value)
          |<-- "new_value" ---------|
```

### Connection-Level Transaction DST: Cross-Connection Conflict

```
  conn_a (SimulatedConnection)    ShardedActorState    conn_b (SimulatedConnection)
          |                              |                        |
          |-- WATCH [k:3] ------------->|                        |
          |<-- OK ----------------------|                        |
          |   (snapshots k:3 value)     |                        |
          |                              |                        |
          |                              |<-- SET k:3="b:42" ----|  (conn_b writes)
          |                              |--- OK --------------->|
          |                              |                        |
          |-- MULTI ------------------->|                        |
          |<-- OK ----------------------|                        |
          |                              |                        |
          |-- SET k:3="a:17" ---------->|                        |
          |<-- QUEUED ------------------|                        |
          |                              |                        |
          |-- EXEC -------------------->|                        |
          |   (compares snapshot to      |                        |
          |    current value of k:3)     |                        |
          |   (mismatch! "b:42" != old)  |                        |
          |<-- Array(None) -------------|          (aborted)     |
```

### CRDT DST: GCounter Convergence

```
  Test            GCounterDSTHarness        Replica 0    Replica 1    Replica 2
   |                     |                      |            |            |
   |-- run(100) -------->|                      |            |            |
   |                     |-- increment_by(r0,5) |            |            |
   |                     |-- increment_by(r1,3)-|----------->|            |
   |                     |-- increment_by(r2,7)-|------------|----------->|
   |                     |   ... 100 random ops |            |            |
   |                     |                      |            |            |
   |-- sync_all() ------>|                      |            |            |
   |                     |   round 1:           |            |            |
   |                     |-- merge(r0, r1) ---->|<---------->|            |
   |                     |-- merge(r0, r2) ---->|<-----------|----------->|
   |                     |-- merge(r1, r2) -----|----------->|<--------->|
   |                     |   ... 5 rounds       |            |            |
   |                     |                      |            |            |
   |-- check_convergence |                      |            |            |
   |                     |-- r0.value() ------->|            |            |
   |                     |-- r1.value() --------|----------->|            |
   |                     |-- r2.value() --------|------------|----------->|
   |                     |   assert all equal   |            |            |
   |<-- result() --------|                      |            |            |
```

### Multi-Node Simulation: Partition and Heal

```
  Test          MultiNodeSimulation     Node 0       Node 1       Node 2
   |                    |                  |            |            |
   |-- partition(0,2) ->|                  |            |            |
   |                    |  [0<->2 blocked] |            |            |
   |                    |                  |            |            |
   |-- execute(1,0,SET) |                  |            |            |
   |                    |-- execute ------>|            |            |
   |                    |                  |            |            |
   |-- gossip_round() ->|                  |            |            |
   |                    |-- drain_deltas ->|            |            |
   |                    |-- send 0->1 ---->|----------->|            |
   |                    |-- send 0->2 ---->|   BLOCKED  |            |
   |                    |                  |            |            |
   |-- heal_partition ->|                  |            |            |
   |   (0, 2)          |  [auto anti-entropy]          |            |
   |                    |-- generate_digest |           |            |
   |                    |   (node 0) ------>|            |            |
   |                    |-- generate_digest |            |            |
   |                    |   (node 2) ------>|------------|----------->|
   |                    |-- divergent_buckets            |            |
   |                    |-- apply_remote_deltas -------->|<--------->|
   |                    |                  |            |            |
   |   (all nodes now consistent)          |            |            |
```

### Stateright Model Check: State Exploration

```
  CrdtMergeModel        Stateright Checker
       |                        |
       |-- init_states() ------>|
       |<-- [CrdtState(empty)] -|
       |                        |
       |   for each state:      |
       |-- actions(state) ----->|
       |<-- [Set{r1,k1,v10},   |
       |     Set{r1,k1,v20},   |
       |     Delete{r1,k1},    |
       |     Sync{from:2,to:1},|
       |     ...]               |
       |                        |
       |   for each action:     |
       |-- next_state(s, a) --->|
       |<-- Some(new_state) ----|
       |                        |
       |-- properties() ------->|
       |<-- [lamport_monotonic, |
       |     tombstone_consist.,|
       |     valid_timestamps]  |
       |                        |
       |   BFS until exhausted  |
       |-- assert_properties -->|
       |<-- PASS (no violations)|
```

---

## File Locations

| Component | File |
|-----------|------|
| Fault constants | `src/buggify/faults.rs` |
| FaultConfig (presets, set/get) | `src/buggify/config.rs` |
| buggify macros, should_buggify, stats | `src/buggify/mod.rs` |
| io::Rng trait, Timestamp | `src/io/mod.rs` |
| SimulatedRng, SimulatedClock, SimulationContext | `src/io/simulation.rs` |
| VirtualTime, Duration | `src/simulator/time.rs` |
| DeterministicRng | `src/simulator/rng.rs` |
| ExecutorDSTHarness, ExecutorDSTConfig | `src/redis/executor_dst.rs` |
| TransactionDSTHarness, TransactionDSTConfig | `src/redis/transaction_dst.rs` |
| Connection-level transaction DST | `tests/connection_transaction_dst.rs` |
| GCounter/PNCounter/ORSet/VectorClock DST | `src/replication/crdt_dst.rs` |
| CRDT types (GCounter, PNCounter, ORSet, VectorClock, LwwRegister) | `src/replication/lattice.rs` |
| Kani proofs | `src/replication/lattice.rs` (`#[cfg(kani)]` block) |
| MultiNodeSimulation | `src/simulator/multi_node.rs` |
| ZipfianGenerator, KeyDistribution, RedisDSTSimulation | `src/simulator/dst_integration.rs` |
| DSTSimulation, DSTConfig, BatchRunner | `src/simulator/dst.rs` |
| Stateright CrdtMergeModel | `src/stateright/replication.rs` |
| TLA+ specifications | `specs/tla/*.tla` |
| Maelstrom KV binary | `src/bin/maelstrom_kv.rs` |
| Maelstrom replicated KV binary | `src/bin/maelstrom_kv_replicated.rs` |
