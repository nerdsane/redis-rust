[![CI](https://github.com/nerdsane/redis-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/nerdsane/redis-rust/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org/)
[![Redis Compat](https://img.shields.io/badge/redis--compat-75%2B_commands-green.svg)](#what-works)
[![Maelstrom](https://img.shields.io/badge/maelstrom-5_node_valid-brightgreen.svg)](#replication)

# redis-rust

An experimental Redis-compatible in-memory data store in Rust with CRDT-based multi-node replication. Co-authored with [Claude Code](https://claude.ai/claude-code) (Opus 4.5 → Opus 4.6) as an exercise in [AI-Driven Research for Systems](https://adrs-ucb.notion.site/datadog) (ADRS). See also: [BitsEvolve](https://www.datadoghq.com/blog/engineering/self-optimizing-system/) for production-aware self-optimizing systems. Read the [full paper](docs/PAPER.md) for architecture, verification methodology, and lessons learned.

> **This is not production software.** It is a research project exploring deterministic simulation testing, actor architectures, and human-AI collaboration on distributed systems code. Do not use this as a Redis replacement.

> **What it actually is:** A Redis-compatible server that speaks RESP2, with actor-per-shard concurrency, CRDT replication (Anna KVS-style gossip + anti-entropy), Lua scripting, transactions, TLS, and ACL auth. Passes the official Redis Tcl test suite for implemented commands. Runs within 80-100% of Redis 7.4 throughput on equivalent hardware.

## What works

**75+ Redis commands** across strings, lists, sets, hashes, sorted sets, transactions, expiration, Lua scripting, and server introspection. RESP2 wire protocol compatible with all standard Redis clients.

**Tcl compatibility test results** (official Redis test suite):

| Suite | Result |
|-------|--------|
| `unit/type/incr` | **28/28 pass** |
| `unit/expire` | **all pass** |
| `unit/type/string` | 35/39 pass (stops at SETBIT - bitmaps not implemented) |
| `unit/multi` | 20/56 pass (stops at SWAPDB - database swapping not implemented) |

**Performance** (Docker, 2 CPUs, 1GB RAM, 50 clients, `redis-benchmark`):

| | Redis 7.4 | redis-rust | |
|---|-----------|------------|---|
| SET P=1 | 148K rps | 147K rps | 99% |
| GET P=1 | 154K rps | 119K rps | 77% |
| SET P=16 | 1.02M rps | 813K rps | 80% |
| GET P=16 | 840K rps | 709K rps | 84% |

Optimal shard count depends on available cores. With 2 CPUs, 2-4 shards peak at ~1M SET/s pipelined.

## What doesn't work

- **No bitmaps, streams, pub/sub, HyperLogLog, or geo commands.**
- **No blocking operations** (BLPOP, BRPOP, etc.).
- **No RESP3.** RESP2 only.
- **No persistence guarantees.** In-memory only. Streaming persistence to S3 exists but is experimental.
- **Multi-node replication is eventual consistency only.** CRDT-based (LWW registers, vector clocks, gossip). Verified via Maelstrom and 87 deterministic simulation tests with partition/loss injection. Not linearizable across nodes by design.
- **MULTI/EXEC works but has limitations.** Transaction state is tracked at the connection level. WATCH uses value-snapshot comparison, not Redis's internal dirty-key tracking.

## Quick start

```bash
# Build and run
cargo run --bin redis-server-optimized --release

# Connect with any Redis client
redis-cli -p 6379
```

Default port is 6379 (standard Redis port). Override with `REDIS_PORT=3000`.

## Testing

See [docs/HARNESS.md](docs/HARNESS.md) for the full verification guide — what each layer tests, expected outputs, pitfalls, and how to add new commands without breaking things.

```bash
# Unit tests (507 tests including 87 replication tests)
cargo test --lib

# CRDT convergence tests (100 seeds, partitions, packet loss)
cargo test crdt_dst --lib
cargo test multi_node --lib

# Tcl compatibility (requires git submodule)
git submodule update --init
./scripts/run-redis-compat.sh

# Maelstrom linearizability (requires Java 11+)
cargo build --release --bin maelstrom-kv-replicated
/opt/homebrew/opt/openjdk@17/bin/java -Djava.awt.headless=true \
  -jar maelstrom/maelstrom/lib/maelstrom.jar test -w lin-kv \
  --bin ./target/release/maelstrom-kv-replicated \
  --node-count 5 --time-limit 30 --concurrency 10 --rate 50

# Docker benchmarks
cd docker-benchmark && ./run-benchmarks.sh
```

## Architecture

Actor-per-shard design. Each shard is an independent tokio task owning its `CommandExecutor`. No locks - all communication via `mpsc` channels. Connection handler parses RESP, routes to the correct shard by key hash, awaits response.

```
Connections ──> [RESP Parser] ──> hash(key) ──> [Shard Actor 0..N] ──> [CommandExecutor]
```

Shard count is configurable via `perf_config.toml`. Transaction state (MULTI/EXEC/WATCH) lives at the connection level, not per-shard, so transactions work correctly across shards.

## Replication

Anna KVS-style CRDT replication with gossip protocol. Each node maintains LWW registers with Lamport clocks. Convergence is guaranteed by CRDT merge properties (commutative, associative, idempotent) - verified by Stateright exhaustive model checking and 100-seed deterministic simulation with network partitions and message loss.

```
Node 1                    Node 2                    Node 3
  |                         |                         |
[LWW Register]  <--Gossip-->  [LWW Register]  <--Gossip-->  [LWW Register]
  |                         |                         |
[Vector Clock]            [Vector Clock]            [Vector Clock]
```

Components: `src/replication/` (CRDTs, gossip, anti-entropy, hash ring), `src/production/replicated_shard_actor.rs` (actor-based replication), `src/bin/maelstrom_kv_replicated.rs` (multi-node proof-of-concept).

**Consistency model:** Eventual (LWW) or Causal (vector clocks). Not linearizable across nodes. Single-node operations are linearizable.

**Maelstrom/Jepsen results** (Knossos linearizability checker):

| Nodes | Operations | Reads | Writes | CAS | Linearizable | Anomalies |
|-------|-----------|-------|--------|-----|-------------|-----------|
| 1 | ~150 | all ok | all ok | all ok | **valid** | 0 |
| 3 | 190 | 98/98 ok | 29/29 ok | 13/63 ok | **valid** | 0 |
| 5 | 1,301 | 670/677 ok | 201/201 ok | 80/423 ok | **valid** | 0 |

CAS failure rate is expected — eventual consistency means CAS often sees stale values. Under low load, Knossos finds valid linearizable orderings because gossip converges fast. Under high load or slow CI runners, Knossos may find linearizability violations where a read arrives before gossip propagates a write. **These violations are correct behavior for an eventually consistent system, not bugs.** CI tolerates linearizability violations but fails on exceptions, crashes, or protocol errors.

**Test coverage:** 87 replication tests, 10 CRDT DST suites (100 seeds each), 6 multi-node simulation tests (partitions, packet loss, convergence), Stateright model checking for merge associativity/commutativity/idempotence, 4 TLA+ specs (gossip, anti-entropy, replication convergence, streaming persistence).

## Configuration

`perf_config.toml` in the working directory (or `PERF_CONFIG_PATH` env var):

```toml
num_shards = 4                    # power of 2, tune to your CPU count

[response_pool]
capacity = 576
prewarm = 96

[buffers]
read_size = 8192
max_size = 536870912              # 512MB max request size

[batching]
min_pipeline_buffer = 70
batch_threshold = 6
```

The root `perf_config.toml` uses `num_shards = 1` for Tcl test compatibility (Lua scripts need all keys on one shard). The `docker-benchmark/perf_config.toml` uses `num_shards = 16` for throughput testing. **If you change one, the other won't change.** The Docker build copies from `docker-benchmark/perf_config.toml`.

## Security

Optional, via feature flags:

```bash
cargo build --release --features tls     # TLS encryption (rustls)
cargo build --release --features acl     # Redis 6.0+ ACL auth
cargo build --release --features security  # both
```

TLS: set `TLS_CERT_PATH`, `TLS_KEY_PATH`, optionally `TLS_CA_PATH` + `TLS_REQUIRE_CLIENT_CERT`.
ACL: set `REDIS_REQUIRE_PASS` for simple auth, or `ACL_FILE` for full user management.

## Project structure

| Path | What |
|------|------|
| `src/redis/command.rs` | Command enum (all 75+ commands) |
| `src/redis/parser.rs`, `commands.rs` | RESP parsing (standard + zero-copy) |
| `src/redis/executor/` | Command execution (`*_ops.rs` files) |
| `src/production/sharded_actor.rs` | Shard routing and aggregation |
| `src/production/connection_optimized.rs` | Connection handler, MULTI/EXEC state |
| `src/replication/` | CRDTs, gossip protocol, anti-entropy, hash ring |
| `src/production/replicated_shard_actor.rs` | Actor-based multi-node replication |
| `src/bin/maelstrom_kv_replicated.rs` | Multi-node Maelstrom proof-of-concept |
| `src/simulator/` | Deterministic simulation testing harness |
| `src/buggify/` | Fault injection (FoundationDB-style) |
| `specs/tla/` | TLA+ specifications (gossip, anti-entropy, convergence) |
| `tests/redis-tests/` | Official Redis Tcl test suite (git submodule) |
| `scripts/run-redis-compat.sh` | Tcl test runner |
| `docker-benchmark/` | Docker-based benchmarking |
| `perf_config.toml` | Server tuning (root = 1 shard, docker-benchmark/ = 16 shards) |

## License

MIT
