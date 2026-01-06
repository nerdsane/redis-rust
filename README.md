# Redis Cache - Deterministic Simulator + Experimental Server

An experimental, actor-based Redis-compatible cache server in Rust with distributed replication capabilities. Features both a deterministic simulator (FoundationDB/TigerBeetle-style testing) and an experimental server using Tiger Style principles with actor-based sharded architecture.

> **Status**: Research project with production-oriented architecture and correctness tooling. Not yet a production Redis replacement. See [Redis Compatibility](#redis-compatibility) for semantic differences.

> **Research Note**: This project is co-authored with [Claude](https://claude.ai) (Anthropic) as an experiment in AI-assisted systems programming. The sole purpose is to explore human-AI collaboration on complex distributed systems code.

> **Security Warning**: This server has **no authentication or access control**. Do NOT expose to untrusted networks or the public internet. Bind to localhost or use network-level access control (firewall, VPC).

## Features

- **Redis-Compatible Server**: Compatible with `redis-cli` and all Redis clients (RESP2 protocol)
- **Tiger Style Engineering**: Explicit over implicit, assertion-heavy, deterministic behavior
- **60+ Redis Commands**: Full caching feature set (strings, lists, sets, hashes, sorted sets)
- **Lua Scripting**: EVAL/EVALSHA with full Redis command access from Lua
- **Dynamic Shard Architecture**: Runtime-configurable shards with lock-free message passing
- **Anna KVS-Style Replication**: Configurable consistency (eventual, causal), coordination-free
- **Hot Key Detection**: Adaptive replication for high-traffic keys
- **Deterministic Simulation**: FoundationDB/TigerBeetle-style testing harness
- **Redis Equivalence Testing**: Differential testing against real Redis (30+ commands verified)
- **Zipfian Workload Simulation**: Realistic hot/cold key access patterns
- **Maelstrom/Jepsen Integration**: Formal linearizability testing (single-node verified)
- **Datadog Observability**: Optional metrics, tracing, and logging via feature flag

## Quick Start

```bash
# Run the optimized production server
cargo run --bin redis-server-optimized --release

# Connect with redis-cli
redis-cli -p 3000

# Run all tests (500+ tests)
cargo test --all

# Run benchmarks
cargo run --bin benchmark --release
```

## Performance vs Redis 8.0

Docker-based benchmark (2 CPUs, 1GB RAM per container, 50 clients, 100K requests):

| Operation | Redis 8.0 | Rust | Relative |
|-----------|-----------|------|----------|
| SET (P=1) | 196,464 req/s | 173,010 req/s | **88%** |
| GET (P=1) | 190,476 req/s | 180,180 req/s | **95%** |
| SET (P=16) | 1,282,051 req/s | 1,098,901 req/s | **86%** |
| GET (P=16) | 1,315,790 req/s | 1,123,596 req/s | **85%** |

Achieves **85-95% of Redis 8.0** out of the box. With [RedisEvolve](evolve/README.md) optimization: **99.1%**.

See [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md) for full details, methodology, and Redis 7.4 comparisons.

## Architecture

### Production Server (Tiger Style)

```
Client Connections
        |
   [Tokio Runtime]
        |
   [Dynamic Shard Actors] <-- Runtime configurable count
        |
   [ShardMessage enum: Command | EvictExpired]
        |
   [CommandExecutor per shard]
```

- **Actor-per-Shard**: Each shard runs as independent actor with message passing
- **Lock-Free**: No RwLock, uses tokio channels for all shard communication
- **Dynamic Sharding**: Runtime-configurable shard count (default: num_cpus)
- **TTL Manager Actor**: Background actor sends eviction messages every 100ms
- **Tiger Style**: `debug_assert!` invariants, explicit error handling, no silent failures

### Performance Optimizations

- **jemalloc Allocator**: ~10% reduced memory fragmentation
- **Actor-per-Shard**: ~30% improvement from lock-free design
- **Buffer Pooling**: ~20% improvement from `crossbeam::ArrayQueue` reuse
- **Zero-copy RESP Parser**: ~15% improvement with `bytes::Bytes` + `memchr`
- **Connection Pooling**: ~10% improvement from semaphore-limited connections

### Anna KVS-Style Replication

```
Node 1                    Node 2                    Node 3
  |                         |                         |
[LWW Register]  <--Gossip-->  [LWW Register]  <--Gossip-->  [LWW Register]
  |                         |                         |
[Lamport Clock]           [Lamport Clock]           [Lamport Clock]
  |                         |                         |
[Vector Clock]            [Vector Clock]            [Vector Clock]
```

- **Configurable Consistency**: `Eventual` (LWW) or `Causal` (vector clocks)
- **CRDT-Based**: Last-Writer-Wins registers with Lamport clocks for total ordering
- **Vector Clocks**: Causal consistency mode tracks happens-before relationships
- **Gossip Protocol**: Periodic state synchronization between nodes
- **Hot Key Detection**: Automatic increased replication for high-traffic keys
- **Anti-Entropy**: Merkle tree-based consistency verification and repair

### Consistency Guarantees

| Mode | Single-Node | Multi-Node |
|------|-------------|------------|
| Linearizable | Yes (verified via Maelstrom) | No |
| Eventual | Yes | Yes (CRDT convergence verified) |
| Causal | Yes | Yes (vector clock verified) |

**Important**: Multi-node mode provides **eventual consistency**, not linearizability. This is by design, following Anna KVS architecture for coordination-free scalability.

## Testing

### Test Suite (500+ tests total)

```bash
# Unit tests
cargo test --lib

# All tests including integration
cargo test --all

# Redis equivalence testing (requires real Redis running on 6379)
cargo test redis_equivalence --release -- --ignored
```

**Test Categories:** Unit tests, Lua scripting, Redis equivalence (30+ commands), CRDT convergence, DST/simulation with fault injection, streaming persistence, and Maelstrom linearizability.

### Redis Equivalence Testing

Differential testing compares our implementation against real Redis to ensure identical behavior:

```bash
# Start real Redis on default port
docker run -d -p 6379:6379 redis:7-alpine

# Start rust implementation
REDIS_PORT=3000 cargo run --bin redis-server-optimized --release &

# Run equivalence tests
cargo test redis_equivalence --release -- --ignored
```

**30+ core commands verified identical** including:
- String operations (GET, SET with NX/XX/EX/PX/GET, APPEND, STRLEN, GETRANGE)
- Numeric operations (INCR, DECR, INCRBY, DECRBY)
- Hash operations (HSET, HGET, HDEL, HGETALL, HINCRBY, HSCAN)
- List operations (LPUSH, RPUSH, LPOP, RPOP, LRANGE, LLEN, LSET, LTRIM, RPOPLPUSH, LMOVE)
- Set operations (SADD, SREM, SMEMBERS, SISMEMBER, SCARD)
- Sorted set operations (ZADD, ZSCORE, ZRANK, ZRANGE, ZCARD, ZCOUNT, ZRANGEBYSCORE, ZSCAN)
- Key operations (DEL, EXISTS, TYPE, EXPIRE, TTL, SCAN)

### Zipfian Workload Simulation

DST tests use Zipfian distribution for realistic hot/cold key access patterns:

```rust
// Top 10 keys receive ~40% of accesses (skew=1.0)
KeyDistribution::Zipfian { num_keys: 100, skew: 1.0 }
```

This simulates real-world workloads where a small subset of keys receive disproportionate traffic.

### Maelstrom/Jepsen Tests

```bash
# Single-node linearizability (PASSES)
./maelstrom/maelstrom/maelstrom test -w lin-kv --bin ./target/release/maelstrom-kv-replicated \
    --time-limit 30 --concurrency 10

# Multi-node eventual consistency
./maelstrom/maelstrom/maelstrom test -w lin-kv --bin ./target/release/maelstrom-kv-replicated \
    --node-count 3 --time-limit 30 --concurrency 10
```

**Note**: Multi-node linearizability test will FAIL because we use eventual consistency. This is expected behavior.

## Supported Commands

### Strings
`GET`, `SET`, `SETEX`, `SETNX`, `MGET`, `MSET`, `APPEND`, `GETSET`, `STRLEN`

### Counters
`INCR`, `DECR`, `INCRBY`, `DECRBY`

### Expiration
`EXPIRE`, `EXPIREAT`, `PEXPIREAT`, `TTL`, `PTTL`, `PERSIST`

### Keys
`DEL`, `EXISTS`, `TYPE`, `KEYS`, `FLUSHDB`, `FLUSHALL`, `SCAN`

### Lists
`LPUSH`, `RPUSH`, `LPOP`, `RPOP`, `LLEN`, `LRANGE`, `LINDEX`, `LSET`, `LTRIM`, `RPOPLPUSH`, `LMOVE`

### Sets
`SADD`, `SREM`, `SMEMBERS`, `SISMEMBER`, `SCARD`

### Hashes
`HSET`, `HGET`, `HDEL`, `HGETALL`, `HKEYS`, `HVALS`, `HLEN`, `HEXISTS`, `HINCRBY`, `HSCAN`

### Sorted Sets
`ZADD`, `ZREM`, `ZSCORE`, `ZRANK`, `ZRANGE`, `ZREVRANGE`, `ZCARD`, `ZCOUNT`, `ZRANGEBYSCORE`, `ZSCAN`

### Server
`PING`, `INFO`

### Scripting
`EVAL`, `EVALSHA`

## Redis Compatibility

### Wire Protocol
| Protocol | Status |
|----------|--------|
| **RESP2** | Full support (compatible with all Redis clients) |
| **RESP3** | Not supported |

### Semantic Differences from Redis

This implementation intentionally differs from Redis in several ways:

| Behavior | Redis | This Implementation | Rationale |
|----------|-------|---------------------|-----------|
| **Multi-node Consistency** | Single-leader strong | Eventual/Causal (CRDT) | Coordination-free scalability |
| **Transactions** | MULTI/EXEC atomic | Not supported | Conflicts with CRDT model |
| **Keyspace Notifications** | Supported | Not supported | Not implemented |
| **Eviction Policies** | LRU/LFU/Random/TTL | TTL-only | Simpler model |
| **Memory Limits** | maxmemory + eviction | No memory limits | Not implemented |
| **Persistence Model** | RDB snapshots / AOF log | Streaming to object store (S3) | Cloud-native design |
| **Cluster Protocol** | Redis Cluster (hash slots) | Anna-style CRDT gossip | Different architecture |
| **Blocking Operations** | BLPOP, BRPOP, etc. | Not supported | Not implemented |

### Not Implemented (No Plans)
These features conflict with the CRDT/eventual consistency architecture:
- **Transactions**: MULTI, EXEC, WATCH, DISCARD
- **Blocking operations**: BLPOP, BRPOP, BLMOVE, etc.
- **Cluster commands**: CLUSTER *, READONLY, READWRITE

### Not Implemented (Roadmap)
These could be added without architectural changes:
- **Pub/Sub**: PUBLISH, SUBSCRIBE, PSUBSCRIBE
- **Streams**: XADD, XREAD, XRANGE, XGROUP
- **Authentication**: AUTH, ACL commands
- **TLS**: Encrypted connections

## Docker Benchmarking

Run fair comparison against official Redis:

```bash
cd docker-benchmark

# In-memory comparison (Redis vs Rust optimized)
./run-benchmarks.sh

# Redis 8.0 comparison (three-way: Redis 7.4 vs Redis 8.0 vs Rust)
./run-redis8-comparison.sh

# Persistent comparison (Redis AOF vs Rust S3/MinIO)
./run-persistent-benchmarks.sh
```

## RedisEvolve - Automatic Performance Tuning

Evolutionary optimization harness that discovers optimal configuration parameters. Achieved **99.1% of Redis 8.0 performance**. See [evolve/README.md](evolve/README.md) for details.

## Observability

Optional Datadog integration (metrics, tracing, logging) via `--features datadog`. See [docs/OBSERVABILITY.md](docs/OBSERVABILITY.md) for setup.

## License

MIT
