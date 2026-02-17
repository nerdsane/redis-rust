---
name: dst
description: Deterministic Simulation Testing methodology, harnesses, fault injection, and shadow state
user_invocable: true
---

# Deterministic Simulation Testing (DST) — redis-rust

You are about to write or modify tests. Section 1 covers the general DST methodology
(pioneered by FoundationDB, adopted by TigerBeetle and Antithesis). Sections 2+ cover
our specific implementation. Use your training knowledge for the general concepts;
trust the file paths and type names below for project specifics.

---

## 1. Why DST (General Theory)

Deterministic Simulation Testing was pioneered by FoundationDB and adopted by
TigerBeetle, Antithesis, and others. The general principle is independent of any
codebase:

**The problem:** Distributed systems have bugs that only manifest under specific
timing, fault sequences, or scheduling orders. Traditional testing samples a tiny
fraction of possible executions. Production bugs hide in the unexplored space.

**The insight:** All non-determinism in a program comes from a finite set of sources:
time, randomness, I/O results, and thread scheduling. If you **replace every source
of non-determinism with a deterministic simulation controlled by a single seed**, then:
- The same seed always produces the same execution
- A bug-triggering seed is a permanent, reproducible test case
- You can explore millions of execution paths by varying seeds
- Fault injection becomes trivial — the simulation controls all I/O

**The contract:** Production code must NEVER access real time, real randomness, or
real I/O directly. Everything goes through an abstraction layer that can be swapped
between production and simulation implementations.

### How we apply this (Project-Specific)

Our sources of non-determinism and how we control them:

| Source | Production | Simulation |
|--------|-----------|------------|
| Time | `SystemTime::now()` | `VirtualTime` (`src/simulator/time.rs`) |
| Randomness | OS RNG | `SimulatedRng` (`src/io/simulation.rs`) |
| I/O | Real network/disk | Simulated with fault injection |
| Scheduling | OS scheduler | Deterministic single-threaded loop |

---

## 2. VirtualTime

**File:** `src/simulator/time.rs`

```rust
pub struct VirtualTime(pub u64);  // millisecond-based
pub struct Duration(pub u64);

impl VirtualTime {
    pub const ZERO: VirtualTime = VirtualTime(0);
    pub fn from_millis(millis: u64) -> Self;
    pub fn from_secs(secs: u64) -> Self;
    pub fn as_millis(&self) -> u64;
}
```

- Millisecond granularity, monotonically increasing
- Executor has `simulation_start_epoch` (seconds) and `simulation_start_epoch_ms` (milliseconds) for epoch-relative timestamps

---

## 3. SimulatedRng

**File:** `src/io/simulation.rs`

Lives inside `SimulationContext`:

```rust
pub struct SimulationContext {
    time: Mutex<Timestamp>,
    clock_offsets: Mutex<HashMap<NodeId, ClockOffset>>,
    rng: Mutex<SimulatedRng>,
    // ...
}
```

Accessed via `SimulatedRuntime::rng()`. Deterministic given the same seed.
Used with `check_buggify()` helper for fault injection decisions.

**Rule:** Never use `rand::thread_rng()` or any production RNG in simulation code.
Always use `SimulatedRng` through the runtime.

---

## 4. Fault Injection (buggify)

**Files:** `src/buggify/faults.rs` (constants), `src/buggify/config.rs` (probabilities)

### All 36 Fault Constants

**Network (8):**
- `network::PACKET_DROP` — Drop packet entirely
- `network::PACKET_CORRUPT` — Corrupt random bytes
- `network::PARTIAL_WRITE` — Truncate packet
- `network::REORDER` — Reorder delivery
- `network::CONNECTION_RESET` — Reset connection
- `network::CONNECT_TIMEOUT` — Timeout on connect
- `network::DELAY` — Significant delay
- `network::DUPLICATE` — Duplicate packet

**Timer (6):**
- `timer::DRIFT_FAST` — Clock runs fast (+1000 ppm)
- `timer::DRIFT_SLOW` — Clock runs slow (-1000 ppm)
- `timer::SKIP` — Skip timer tick
- `timer::DUPLICATE` — Duplicate timer tick
- `timer::JUMP_FORWARD` — Large clock jump forward
- `timer::JUMP_BACKWARD` — Small clock jump backward (NTP correction)

**Process (5):**
- `process::CRASH` — Crash process
- `process::PAUSE` — Pause execution
- `process::SLOW` — Slow processing
- `process::OOM` — Out of memory
- `process::CPU_STARVATION` — Scheduling delays

**Disk (7):**
- `disk::WRITE_FAIL` — Write fails
- `disk::PARTIAL_WRITE` — Partial write
- `disk::CORRUPTION` — Data corruption
- `disk::SLOW` — Slow I/O
- `disk::FSYNC_FAIL` — fsync fails
- `disk::STALE_READ` — Returns stale data
- `disk::DISK_FULL` — Disk full

**Object Store (9):**
- `object_store::PUT_FAIL` — Put fails
- `object_store::GET_FAIL` — Get fails
- `object_store::GET_CORRUPT` — Get returns corrupted data
- `object_store::TIMEOUT` — Operation times out
- `object_store::PARTIAL_WRITE` — Segment truncated
- `object_store::DELETE_FAIL` — Delete fails
- `object_store::LIST_INCOMPLETE` — List returns incomplete results
- `object_store::RENAME_FAIL` — Rename fails (non-atomic)
- `object_store::SLOW` — Slow response

**Replication (5):**
- `replication::GOSSIP_DROP` — Drop gossip message
- `replication::GOSSIP_DELAY` — Delay gossip
- `replication::GOSSIP_CORRUPT` — Corrupt gossip payload
- `replication::SPLIT_BRAIN` — Split brain scenario
- `replication::STALE_REPLICA` — Stale replica response

### Preset Configurations

**File:** `src/buggify/config.rs`

```rust
pub struct FaultConfig {
    pub enabled: bool,
    pub probabilities: HashMap<&'static str, f64>,
    pub global_multiplier: f64,
}
```

| Preset | Multiplier | Use case |
|--------|-----------|----------|
| `FaultConfig::disabled()` | 0.0 | Baseline correctness tests |
| `FaultConfig::calm()` | 0.1 | Light fault injection |
| `FaultConfig::moderate()` | 1.0 | Default for DST (balanced) |
| `FaultConfig::chaos()` | 3.0 | Stress testing |

Key methods:
- `set(fault_id, probability)` — set per-fault probability (clamped 0.0-1.0)
- `get(fault_id) -> f64` — get probability
- `should_trigger(fault_id, random_value) -> bool` — check if fault fires

---

## 5. DST Harness Files

### Executor-level DST

| Harness file | Test file | What it tests |
|-------------|-----------|---------------|
| `src/redis/executor_dst.rs` | `tests/executor_dst_test.rs` | String, key, and general commands |
| `src/redis/list_dst.rs` | `tests/list_dst_test.rs` | List commands (LPUSH, RPUSH, LPOP, etc.) |
| `src/redis/set_dst.rs` | `tests/set_dst_test.rs` | Set commands (SADD, SREM, SMEMBERS, etc.) |
| `src/redis/hash_dst.rs` | `tests/hash_dst_test.rs` | Hash commands (HSET, HGET, HDEL, etc.) |
| `src/redis/sorted_set_dst.rs` | `tests/sorted_set_dst_test.rs` | Sorted set commands (ZADD, ZRANGE, etc.) |
| `src/redis/transaction_dst.rs` | `tests/transaction_dst_test.rs` | MULTI/EXEC/DISCARD/WATCH |

### Security DST

| Harness file | Test file | What it tests |
|-------------|-----------|---------------|
| `src/security/acl_dst.rs` | `tests/acl_dst_test.rs` | ACL user management, auth, command/key permissions (`--features acl`) |

### Replication / Streaming DST

| Harness file | Test file | What it tests |
|-------------|-----------|---------------|
| `src/replication/crdt_dst.rs` | `tests/crdt_dst_test.rs` | CRDT merge properties under fault injection |
| `src/streaming/compaction_dst.rs` | `tests/streaming_dst_test.rs` | Streaming persistence with faults |

### Other DST tests

| Test file | What it tests |
|-----------|---------------|
| `tests/cluster_config_dst_test.rs` | Cluster configuration under faults |

---

## 6. Shadow State Pattern

The core DST pattern: maintain a **shadow** (reference model) alongside the real
implementation. After each operation, assert the real state matches the shadow.

```
                  ┌─────────────────┐
  Command ──────> │ CommandExecutor  │ ──> Real result
                  └─────────────────┘
                  ┌─────────────────┐
  Same command ─> │  ShadowState    │ ──> Expected result
                  └─────────────────┘
                          |
                    Assert equal
```

The shadow is a simple HashMap/data structure that implements the same semantics as Redis
but with zero optimization — it's obviously correct.

### Example from executor_dst.rs

```rust
// Shadow state: simple HashMap<String, ShadowValue>
// After each command:
let real_result = executor.execute(cmd);
let expected = shadow.apply(cmd);
assert_eq!(real_result, expected, "Mismatch for seed {seed}, op {i}");
```

---

## 7. Borrow Checker Traps in DST Harnesses

**Problem:** Cannot call `self.assert_*()` (takes `&mut self`) while holding a reference
from `self.shadow.get()` (borrows `&self`).

**Fix:** Extract the expected value into a local variable first.

```rust
// BAD: borrow conflict
let expected = self.shadow.get(&key);  // borrows &self
self.assert_result(result, expected);   // borrows &mut self  -- ERROR!

// GOOD: extract first
let expected = self.shadow.get(&key).cloned();  // local value, borrow ends
self.assert_result(result, expected);             // fine now
```

For enums, extract into a local enum/variable before asserting.

---

## 8. How to Add DST Coverage for a New Command

**This is step 5 in the command addition checklist** (see `/rust-dev` section 13 and
`docs/HARNESS.md`). DST coverage is required for every new command.

1. **Identify the category** — is it a string/list/set/hash/sorted_set/transaction command?

2. **Open the appropriate harness** — e.g., `src/redis/executor_dst.rs` for string/key
   commands, `src/redis/list_dst.rs` for list commands, etc.

3. **Add a new operation variant** to the random operation generator. For executor_dst.rs,
   this means adding a branch in the appropriate `run_*_op()` method with a probability
   allocation from the `sub` range:
   ```rust
   // In run_string_op(), carve a % from the sub range:
   } else {
       // YOUR_NEW_COMMAND
       let key = self.random_key();
       let resp = self.executor.execute(&Command::YourNewCommand(key.clone(), ...));
       // Assert against shadow
   }
   ```

4. **Update the shadow state** to track the expected result of your command.

5. **Add assertion** comparing real executor result to shadow expectation. Use the
   extract-then-assert pattern (see section 7) to avoid borrow checker conflicts.

6. **Run with multiple seeds:**
   ```bash
   cargo test --release --test executor_dst_test -- --nocapture
   ```
   Minimum 10 seeds, prefer 50+.

7. **If adding fields to `Command::Set`** (or similar large enum variant), update ALL
   ~25+ struct literal constructions across test files. Search: `Command::Set {`.

---

## 9. Running DST Tests

```bash
# All DST tests
cargo test dst --release -- --nocapture

# Specific category
cargo test --lib executor_dst -- --nocapture
cargo test --lib list_dst -- --nocapture
cargo test --lib set_dst -- --nocapture
cargo test --lib hash_dst -- --nocapture
cargo test --lib sorted_set_dst -- --nocapture
cargo test --lib transaction_dst -- --nocapture

# CRDT DST
cargo test crdt_dst -- --nocapture

# Streaming DST
cargo test streaming_dst -- --nocapture

# ACL DST (requires --features acl)
cargo test --lib --features acl acl_dst -- --nocapture
cargo test --test acl_dst_test --features acl -- --nocapture

# With specific seed (when reproducing a failure)
# Seeds are typically passed as test parameters, check each harness
```

---

## 10. Seed Fragility

Seeds encode a path through the code. If you change the code, the path changes, and
old seeds may explore different states. This is expected.

- If a previously-passing seed fails after your change: **debug the failure and fix the bug.**
- Do NOT delete the test.
- Do NOT change the seed to make it pass.
- The failure likely reveals a real bug in your change.

---

## 11. Zipfian Workload Generation

**File:** `src/simulator/dst_integration.rs`

Use Zipfian distribution for realistic key access patterns:

```rust
KeyDistribution::Zipfian { num_keys: 100, skew: 1.0 }
```

With skew=1.0, top 10 keys get ~40% of accesses (matches real Redis workloads).
Always prefer Zipfian over uniform for DST tests.

---

## Anti-patterns

- **Using production RNG in simulation.** Always use `SimulatedRng`. Production RNG breaks determinism.
- **Skipping shadow state.** Every DST test MUST have a reference model. Without shadow state, you're testing "does it crash" not "does it compute correctly."
- **Single seed.** Run minimum 10 seeds, prefer 50+. A single seed explores one path.
- **Not testing with fault injection.** Use at least `FaultConfig::moderate()`. Without faults, you're doing integration testing, not DST.
- **Ignoring DST failures.** DST failures are real bugs. Fix the code, not the test.
- **Using `std::time` in executor code.** Use `VirtualTime`. Wall-clock time breaks simulation determinism.
