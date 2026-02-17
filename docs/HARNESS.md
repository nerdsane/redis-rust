# Verification Harness

How to verify this codebase is correct. Written for AI agents and human engineers who didn't write the code.

> This project was co-authored with AI. The harness exists because **AI-generated code must be verified by something other than the AI that wrote it.** Every claim in the README is backed by a runnable command below.

## The verification loop

Every change must survive all four layers. Run them in order. If any layer fails, stop and fix before proceeding.

```
Layer 1: cargo test --lib                              # unit tests (< 70s)
Layer 1b: cargo test --release wal                     # WAL DST tests (multi-seed)
Layer 2: ./scripts/run-redis-compat.sh                 # Official Redis Tcl suite (< 60s)
Layer 3: Maelstrom linearizability check               # Jepsen/Knossos (< 60s)
Layer 4: docker-benchmark/run-benchmarks.sh            # Performance regression (< 5min)
```

Layer 1 is mandatory for every change. Layer 2 for any command behavior change. Layer 3 for any replication change. Layer 4 only for performance claims.

## Layer 1: Unit tests

```bash
cd /path/to/redis-rust-main
cargo test --lib
```

**Expected:** `507 passed; 0 failed; 3 ignored`

The 3 ignored are doc tests. If any test fails, the change is broken. Do not skip tests, do not use `--skip`, do not add `#[ignore]`.

**What's in the 507:**
- 87 replication tests (CRDT convergence, gossip, anti-entropy)
- 10 CRDT DST suites at 100 seeds each (partition injection, packet loss)
- 6 multi-node simulation tests
- 5 executor DST tests (command correctness under fault injection)
- ~400 unit tests (data structures, parsing, execution, scanning, transactions, Lua, ACL, streaming)

**Subset runs for faster iteration:**
```bash
cargo test --lib executor_dst          # Command executor correctness
cargo test --lib crdt_dst              # CRDT convergence
cargo test --lib multi_node            # Multi-node replication
cargo test --lib replicat              # All 87 replication tests
cargo test --release wal               # WAL durability + group commit DST
```

## Layer 2: Redis Tcl compatibility

```bash
git submodule update --init            # First time only
./scripts/run-redis-compat.sh
```

**Expected output:**
```
unit/type/incr:   ALL PASS (28/28)
unit/expire:      ALL PASS
unit/type/string: 72 pass, 0 errors, crash at LCS (not implemented)
unit/multi:       20/56 pass, crash at SWAPDB (database swap not implemented)
```

The remaining failures are from unimplemented commands (LCS, SWAPDB), not bugs. The harness crashes on the first unimplemented command in a file, so all subsequent tests in that file are skipped.

**Run a single suite:**
```bash
./scripts/run-redis-compat.sh unit/type/incr     # Just incr tests
./scripts/run-redis-compat.sh unit/expire         # Just expire tests
```

### Tcl harness pitfalls (read this before touching command code)

**The harness will crash the entire test file on any unknown command.** One `ERR unknown command 'FOO'` kills all remaining tests in that file. A stub that returns an error is better than no parsing at all.

**Error messages must match exactly.** The Tcl tests use `assert_error "*pattern*"` with glob matching. Redis uses specific formats:
```
ERR wrong number of arguments for 'xxx' command
ERR value is not an integer or out of range
ERR value is not a valid float
ERR syntax error
ERR invalid expire time in 'xxx' command
WRONGTYPE Operation against a key holding the wrong kind of value
EXECABORT Transaction discarded because of previous errors.
```

**Double ERR prefix.** `connection_optimized.rs::encode_error_into()` prepends `ERR ` to error strings. If your parser already returns a string starting with `ERR `, the client sees `ERR ERR ...`. The function checks for this, but stay aware.

**Two config files.** The root `perf_config.toml` has `num_shards = 1` (required for Tcl tests — Lua scripts and MULTI/EXEC need all keys on one shard). The `docker-benchmark/perf_config.toml` has `num_shards = 16` (for throughput). The Docker build copies from `docker-benchmark/`. **Do not change root to multi-shard without understanding the consequences.**

**Buffer size.** `max_size` in `perf_config.toml` limits the maximum request size. Set to 512MB. If a Tcl test sends a 4MB payload and the buffer is smaller, the server drops the connection and the harness reports "broken pipe."

**MULTI/EXEC is connection-level.** Transaction state lives in `connection_optimized.rs`, not in the per-shard executor. The executor has its own transaction state for the DST/simulation path, but the production server intercepts MULTI/EXEC/DISCARD/WATCH before shard routing.

**Shard-aggregated commands.** These commands fan out to all shards in `sharded_actor.rs`: DBSIZE, SCAN, KEYS, EXISTS, DEL, FLUSHDB, FLUSHALL, MGET, MSET. If you add a command that needs to see all keys, add aggregation there.

## Layer 3: Maelstrom linearizability

Requires Java 11+. Uses the official [Maelstrom](https://github.com/jepsen-io/maelstrom) workbench by Kyle Kingsbury (Jepsen).

```bash
cargo build --release --bin maelstrom-kv-replicated

# Single-node (should always pass)
/opt/homebrew/opt/openjdk@17/bin/java -Djava.awt.headless=true \
  -jar maelstrom/maelstrom/lib/maelstrom.jar test -w lin-kv \
  --bin ./target/release/maelstrom-kv-replicated \
  --node-count 1 --time-limit 10 --concurrency 4

# Multi-node (5 nodes, stress)
/opt/homebrew/opt/openjdk@17/bin/java -Djava.awt.headless=true \
  -jar maelstrom/maelstrom/lib/maelstrom.jar test -w lin-kv \
  --bin ./target/release/maelstrom-kv-replicated \
  --node-count 5 --time-limit 30 --concurrency 10 --rate 50
```

**Expected for 1-node:** `Everything looks good!` — single-node is always linearizable.

**Expected for 5-node:** Either `Everything looks good!` or `Analysis invalid!` with `:linearizable {:valid? false}`. Both are acceptable. The system uses eventual consistency — under high load, reads may arrive before gossip propagates writes, producing valid linearizability violations. **CI tolerates linearizability violations but fails on exceptions, crashes, or protocol errors.**

**What the test checks:**
- `:exceptions {:valid? true}` — no crashes or protocol errors (MUST pass)
- `:timeline {:valid? true}` — message delivery worked (MUST pass)
- `:linearizable {:valid? true/false}` — linearizable ordering exists (MAY fail for eventual consistency)

**Concurrency must be a multiple of 2.** Maelstrom's `lin-kv` workload uses 2 threads per independent key. With N nodes, use `--concurrency N*2` or any even number >= 2.

**CAS failure rate will be high.** This is expected — the system uses eventual consistency (CRDT gossip), so CAS often reads stale values. Knossos checks whether a valid linearization exists for the *observed* history, not whether every CAS succeeds.

**What the test proves:** The gossip protocol correctly routes messages, applies deltas, and merges CRDT state. Under low load, convergence is fast enough to appear linearizable. Under high load, linearizability violations confirm the system is eventually consistent as designed — not a bug.

**Results are stored in:** `store/lin-kv/<timestamp>/` with `results.edn`, `history.txt`, latency/rate PNGs, and timeline HTML.

## Layer 4: Docker benchmarks

```bash
cd docker-benchmark
./run-benchmarks.sh
```

**Expected:** Rust implementation within 80-100% of Redis 7.4 for SET/GET at P=1 and P=16. No errors, no crashes, full 100K requests processed for each benchmark.

**Port conflict.** The compose file exposes Redis 7.4 on port 6379. If something else is using that port, either stop it or temporarily edit `docker-compose.yml`.

**Config matters.** The Dockerfile copies `docker-benchmark/perf_config.toml` (16 shards). If you accidentally change this to 1 shard, pipelined throughput drops ~60%.

**Performance numbers are noisy.** Docker benchmarks on laptops vary 10-20% between runs. Don't chase single-digit percentage changes. Run 3 times and take the median for any claim.

## Adding a new command

Checklist. Every step is required or the Tcl harness will crash:

1. **`src/redis/command.rs`** — Add enum variant. Update ALL match arms: `get_primary_key()`, `get_keys()`, `name()`, `is_read_only()`.

2. **`src/redis/parser.rs` AND `src/redis/commands.rs`** — Add parsing in BOTH. They must stay in sync. The first is for the standard RESP parser, the second is the zero-copy parser used by production connections. Use `map_err` on parse calls to return Redis-compatible error strings (e.g., `ERR bit offset is not an integer or out of range`), never raw Rust parse errors which will fail Tcl `assert_error` glob matching.

3. **`src/redis/executor/*_ops.rs`** — Implement the logic. Follow the existing pattern: `pub(super) fn execute_xxx(&mut self, ...) -> RespValue`.

4. **`src/redis/executor/mod.rs`** — Add dispatch arm in `execute()`.

5. **`src/redis/executor_dst.rs`** — Add DST coverage with shadow state in the appropriate `run_*_op()` method. Track expected results in the shadow and assert against executor results.

6. **If the command needs all keys** (like DBSIZE, SCAN) — add shard fan-out in `src/production/sharded_actor.rs`.

7. **If you add a field to `Command::Set`** — update ALL ~25+ struct literal constructions across test files, `script_ops.rs`, etc. Use `cargo check --all-targets` to find them all.

8. **Error messages** — Use Redis-standard format. The Tcl tests glob-match on error strings.

9. **Run layers 1 and 2** before considering it done.

## What the tests do NOT cover

- Real network partitions (Maelstrom uses simulated instant delivery)
- Persistence durability (streaming to S3 is experimental)
- Memory limits / eviction policies (not implemented)
- Cluster rebalancing (not implemented)
- RESP3 protocol
- Bitmaps, streams, pub/sub, HyperLogLog, geo commands

## Reproducing a failure

Unit test failures are deterministic. DST tests use seeds — the test name includes the seed. Re-run the specific test to reproduce:

```bash
cargo test --lib test_executor_dst_single_seed    # Exact same seed every time
cargo test --lib test_gcounter_dst_100_seeds       # 100 deterministic seeds
```

Tcl test failures depend on server state. The harness starts a fresh server per file. If a test fails, check:
1. Server logs: `/tmp/redis-rust-compat-server.log`
2. Which command crashed: the exception message names the exact command
3. Whether it's an unimplemented command (expected) or a behavior bug (fix it)

Maelstrom failures store full history in `store/lin-kv/<timestamp>/history.txt`. Read it to see exactly which operation violated linearizability.
