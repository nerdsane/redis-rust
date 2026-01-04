# Redis Server Performance Benchmark Results

## Test Configuration

**Server:** Tiger Style Redis Server (Actor-per-Shard Architecture)
**Binary:** `redis-server-optimized`
**Port:** 3000
**Shards:** Dynamic (default: num_cpus)
**Date:** January 3, 2026

### System Configuration

| Component | Specification |
|-----------|---------------|
| CPU | Intel Core i9-11950H @ 2.60GHz (8 cores, 16 threads) |
| RAM | 32 GB DDR4 |
| OS | Ubuntu 22.04 (Linux 6.8.0-86-generic) |
| Rust | 1.87.0-nightly (f9e0239a7 2025-03-04) |
| Cargo | 1.87.0-nightly (2622e844b 2025-02-28) |

## Fair Comparison: Docker Benchmark vs Official Redis 7.4

To ensure a fair comparison, we run both servers in identical Docker containers with resource limits.

### Docker Configuration

| Setting | Value |
|---------|-------|
| CPU Limit | 2 cores per container |
| Memory Limit | 1GB per container |
| Network | Host networking |
| Requests | 100,000 |
| Clients | 50 concurrent |
| Pipeline | 1 (non-pipelined) |
| Data Size | 64 bytes |

### Non-Pipelined Performance (Pipeline=1)

| Operation | Official Redis 7.4 | Rust Implementation | Relative |
|-----------|-------------------|---------------------|----------|
| SET | 79,051 req/sec | 75,988 req/sec | **96%** |
| GET | 76,570 req/sec | 76,687 req/sec | **100%** |
| INCR | 78,431 req/sec | 71,788 req/sec | **92%** |

**Conclusion:** Our implementation achieves **92-100% of Redis 7.4 performance** on single operations.

### Pipelined Performance (Pipeline=16)

| Operation | Official Redis 7.4 | Rust Implementation | Relative |
|-----------|-------------------|---------------------|----------|
| SET | 806,452 req/sec | **925,926 req/sec** | **+15%** |
| GET | 877,193 req/sec | **909,091 req/sec** | **+4%** |
| INCR | 787,402 req/sec | **990,099 req/sec** | **+26%** |

**Result:** Our implementation is **4-26% FASTER than Redis 7.4** on pipelined workloads!

### High Pipeline Performance (Pipeline=64)

| Operation | Official Redis 7.4 | Rust Implementation | Relative |
|-----------|-------------------|---------------------|----------|
| SET | 1,020,735 req/sec | **1,563,000 req/sec** | **+53%** |
| GET | 1,538,954 req/sec | 1,449,739 req/sec | 94% |

**Result:** At high pipeline depths, SET operations are **53% faster** than Redis 7.4!

**Why we're faster on pipelining:**
1. Batched response flushing (single syscall for all responses)
2. TCP_NODELAY enabled for lower latency
3. Lock-free actor architecture handles concurrent requests efficiently
4. Zero-copy RESP parsing with `bytes::Bytes`

## Local Benchmark (Single Machine, No Resource Limits)

### Optimized Server (`redis-server-optimized`)

| Command | Throughput | Latency | Notes |
|---------|------------|---------|-------|
| PING | 364,289 req/sec | 0.003 ms | Baseline |
| SET | 343,784 req/sec | 0.003 ms | Write path |
| GET | 166,818 req/sec | 0.006 ms | Read path |
| INCR | 314,598 req/sec | 0.003 ms | Atomic counter |
| MSET (5 keys) | 75,043 req/sec | 0.013 ms | Multi-key write |

**Note:** Local benchmarks run without resource constraints and networking overhead. Docker benchmarks provide a fairer comparison.

### Performance Optimization Stack

| Optimization | Description | Improvement |
|-------------|-------------|-------------|
| jemalloc | `tikv-jemallocator` custom allocator | ~10% |
| Actor-per-Shard | Lock-free tokio channels (no RwLock) | ~30% |
| Buffer Pooling | `crossbeam::ArrayQueue` buffer reuse | ~20% |
| Zero-copy Parser | `bytes::Bytes` + `memchr` RESP parsing | ~15% |
| Connection Pooling | Semaphore-limited with shared buffers | ~10% |

### Performance Evolution

| Version | Architecture | Throughput | Key Change |
|---------|-------------|------------|------------|
| v1 (baseline) | Single Lock | ~15,000 req/sec | Initial implementation |
| v2 (sharded) | 16 Shards + RwLock | ~25,000 req/sec | +67% from sharding |
| v3 (optimized) | Actor-per-Shard | ~80,000 req/sec | Lock-free design |
| v4 (fair test) | Docker constrained | ~80,000 req/sec | Matches Redis 7.4 |

## Architecture Details

### Actor-per-Shard Design

```
Client Connection
       |
  [Connection Handler]
       |
  hash(key) % num_shards
       |
  [ShardActor 0..N]  <-- tokio::mpsc channels (lock-free)
       |
  [CommandExecutor]
```

- **Lock-Free**: No `RwLock` contention between shards
- **Dynamic Shards**: Runtime-configurable shard count
- **Message Passing**: Explicit `ShardMessage` enum routes commands
- **TTL Manager**: Separate actor sends `EvictExpired` messages

### Zero-Copy RESP Parser

```
[RespCodec::parse]
       |
  [memchr] for CRLF scanning
       |
  [bytes::Bytes] zero-copy slicing
       |
  [RespValueZeroCopy] borrowed references
```

- **No Allocations**: Parser borrows from input buffer
- **Fast Scanning**: `memchr` SIMD-optimized byte search
- **Incremental**: Handles partial reads efficiently

## Comparison with Official Redis 7.4

| Feature | Official Redis 7.4 | This Implementation | Notes |
|---------|-------------------|---------------------|-------|
| **Performance (SET P=1)** | 79,051 req/sec | 75,988 req/sec | 96% of Redis |
| **Performance (GET P=1)** | 76,570 req/sec | 76,687 req/sec | 100% of Redis |
| **Performance (INCR P=1)** | 78,431 req/sec | 71,788 req/sec | 92% of Redis |
| **Pipelining SET (P=16)** | 806,452 req/sec | **925,926 req/sec** | **+15% FASTER** |
| **Pipelining GET (P=16)** | 877,193 req/sec | **909,091 req/sec** | **+4% FASTER** |
| **Pipelining INCR (P=16)** | 787,402 req/sec | **990,099 req/sec** | **+26% FASTER** |
| **High Pipeline SET (P=64)** | 1,020,735 req/sec | **1,563,000 req/sec** | **+53% FASTER** |
| Persistence (RDB/AOF) | Yes | No | Trade-off |
| Clustering | Redis Cluster | Anna-style CRDT | Different model |
| Consistency | Strong (single-leader) | Eventual/Causal | Trade-off |
| Pub/Sub | Yes | No | Not implemented |
| Lua Scripting | Yes | No | Not implemented |
| Memory Safety | Manual C | Rust guarantees | Safer |
| Deterministic Testing | No | DST framework | Better testability |
| Hot Key Detection | Manual | Automatic | Better |

### Trade-offs

**What we sacrifice:**
- Persistence (RDB/AOF)
- Pub/Sub, Streams, Lua scripting
- Strong consistency in multi-node

**What we gain:**
- **FASTER pipelining** (4-26% faster at P=16, up to 53% faster at P=64)
- Memory safety via Rust
- Coordination-free replication (Anna-style)
- Deterministic simulation testing
- Automatic hot key detection
- Configurable consistency (eventual/causal)

## Replication Performance

| Mode | Throughput | Notes |
|------|------------|-------|
| Single-node | ~80,000 req/sec | No replication overhead |
| Replicated (3 nodes, eventual) | ~64,000 req/sec (est.) | With gossip synchronization |
| Replication Overhead | ~20% | Delta capture + gossip |

### Replication Features

- **Coordination-free**: No consensus protocol for writes
- **Conflict Resolution**: LWW registers with Lamport clocks
- **Consistency Modes**: Eventual (LWW) or Causal (vector clocks)
- **Gossip Interval**: 100ms (configurable)
- **Hot Key Detection**: Automatic RF increase for high-traffic keys
- **Anti-Entropy**: Merkle tree-based consistency verification

## Correctness Testing

### Test Suite (219 tests)

| Category | Tests | Coverage |
|----------|-------|----------|
| Unit Tests | 138 | RESP parsing, commands, data structures |
| Eventual Consistency | 9 | CRDT convergence, partition healing |
| Causal Consistency | 10 | Vector clocks, read-your-writes |
| DST/Simulation | 5 | Multi-seed chaos testing |
| Anti-Entropy | 8 | Merkle tree sync, split-brain |
| Hot Key Detection | 5 | Adaptive replication |
| Metrics Service | 26 | CRDT counters, gauges, distributions |
| Integration | 18 | End-to-end scenarios |

### Maelstrom/Jepsen Results

| Test | Nodes | Result | Notes |
|------|-------|--------|-------|
| Linearizability (lin-kv) | 1 | **PASS** | Single-node is linearizable |
| Linearizability (lin-kv) | 3 | **FAIL** | Expected: eventual consistency |
| Linearizability (lin-kv) | 5 | **FAIL** | Expected: eventual consistency |

**Note:** Multi-node linearizability tests FAIL by design. We use Anna-style eventual consistency, not Raft/Paxos consensus.

## Running Benchmarks

### Docker Benchmark (Recommended for Fair Comparison)

```bash
cd docker-benchmark
./run-benchmarks.sh
```

### Local Benchmark

```bash
# Run optimized server
cargo run --bin redis-server-optimized --release

# Run internal benchmarks
cargo run --bin benchmark --release
```

### Run Tests

```bash
# All tests (219)
cargo test --all

# Unit tests only
cargo test --lib
```

## Conclusion

The Tiger Style Redis server demonstrates:

- **92-100% of Redis 7.4 performance** on single operations
- **4-26% FASTER than Redis 7.4** on pipelined workloads (P=16)
- **Up to 53% FASTER** at high pipeline depths (P=64)
- **1,563,000 req/sec peak throughput** (pipelined SET at P=64)
- **Sub-millisecond latency** (0.003-0.006 ms average)
- **Memory-safe** Rust implementation with no data races
- **Deterministic testability** via FoundationDB-style simulation
- **Single-node linearizability verified** via Maelstrom/Jepsen testing
- **219 tests** covering consistency, replication, and chaos scenarios

### Known Limitations

1. **No persistence**: In-memory only (acceptable for caching)
2. **No pub/sub**: Not implemented
3. **Multi-node consistency**: Eventual, not linearizable (by design)
