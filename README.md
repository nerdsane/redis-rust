# Redis Cache - Production Server + Deterministic Simulator

A production-ready, actor-based Redis cache server in Rust with distributed replication capabilities. Features both a deterministic simulator (FoundationDB-style testing) and a real production server using sharded actor-based architecture with Anna KVS-inspired replication.

## Features

- **Production Redis Server**: Compatible with `redis-cli` and all Redis clients
- **35+ Redis Commands**: Full caching feature set (strings, lists, sets, hashes, sorted sets)
- **16-Shard Architecture**: Hash-partitioned keyspace for parallel command execution
- **Anna KVS Replication**: Coordination-free, eventually-consistent multi-node deployments
- **Deterministic Simulator**: FoundationDB-style testing for correctness verification
- **Maelstrom/Jepsen Integration**: Formal linearizability testing

## Quick Start

```bash
# Run the production server
cargo run --bin redis-server --release

# Connect with redis-cli
redis-cli -p 3000

# Run benchmarks
cargo run --bin benchmark --release

# Run tests
cargo test
```

## Architecture

### Production Server

```
Client Connections
        |
   [Tokio Runtime]
        |
   [16 Sharded Executors]
        |
   [RwLock per Shard]
        |
   [CommandExecutor + ReplicaState]
```

- **Actor-Based**: Each client connection runs as an independent async task
- **Sharding**: Keys are hash-partitioned across 16 independent executors
- **Thread-Safe**: parking_lot RwLock for efficient concurrent access
- **TTL Expiration**: Background actor handles key expiration every 100ms

### Anna KVS-Style Replication

```
Node 1                    Node 2                    Node 3
  |                         |                         |
[LWW Register]  <--Gossip-->  [LWW Register]  <--Gossip-->  [LWW Register]
  |                         |                         |
[Lamport Clock]           [Lamport Clock]           [Lamport Clock]
```

- **CRDT-Based**: Last-Writer-Wins registers with Lamport clocks for conflict resolution
- **Vector Clocks**: Optional causal consistency tracking
- **Gossip Protocol**: Periodic state synchronization between nodes
- **Configurable Consistency**: Eventual (default) or Causal

## Performance

| Operation | Throughput | Latency |
|-----------|------------|---------|
| PING | ~25,000 req/sec | 0.04 ms |
| SET | ~23,500 req/sec | 0.04 ms |
| GET | ~22,000 req/sec | 0.05 ms |
| INCR | ~24,000 req/sec | 0.04 ms |

Tested with 25 concurrent clients, 16 shards. See [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md) for details.

## Supported Commands

### Strings
`GET`, `SET`, `SETEX`, `SETNX`, `MGET`, `MSET`, `APPEND`, `GETSET`, `STRLEN`

### Counters
`INCR`, `DECR`, `INCRBY`, `DECRBY`

### Expiration
`EXPIRE`, `EXPIREAT`, `PEXPIREAT`, `TTL`, `PTTL`, `PERSIST`

### Keys
`DEL`, `EXISTS`, `TYPE`, `KEYS`, `FLUSHDB`, `FLUSHALL`

### Lists
`LPUSH`, `RPUSH`, `LPOP`, `RPOP`, `LLEN`, `LRANGE`, `LINDEX`

### Sets
`SADD`, `SREM`, `SMEMBERS`, `SISMEMBER`, `SCARD`

### Hashes
`HSET`, `HGET`, `HDEL`, `HGETALL`, `HKEYS`, `HVALS`, `HLEN`, `HEXISTS`

### Sorted Sets
`ZADD`, `ZREM`, `ZSCORE`, `ZRANK`, `ZRANGE`, `ZCARD`

### Server
`PING`, `INFO`

## Correctness Testing

The project includes Maelstrom integration for formal correctness testing:

```bash
# Run Maelstrom tests
./scripts/maelstrom_test.sh
```

**Test Results:**
- Single-node linearizability: PASS
- Multi-node (3 nodes) with replication: PASS

## Project Structure

```
src/
├── bin/
│   ├── server.rs              # Production server entry point
│   ├── benchmark.rs           # Performance benchmarks
│   ├── maelstrom_kv.rs        # Single-node Maelstrom binary
│   └── maelstrom_kv_replicated.rs  # Multi-node Maelstrom binary
├── redis/
│   ├── commands.rs            # Command executor and data structures
│   ├── resp.rs                # RESP protocol parser
│   └── sds.rs                 # Simple Dynamic Strings
├── production/
│   ├── server.rs              # Async server implementation
│   ├── sharded_state.rs       # 16-shard state management
│   ├── replicated_state.rs    # Replicated sharded state
│   └── gossip_manager.rs      # Gossip protocol networking
├── replication/
│   ├── lattice.rs             # CRDT primitives (LWW, VectorClock)
│   ├── state.rs               # Replica state management
│   ├── gossip.rs              # Gossip message types
│   └── config.rs              # Replication configuration
└── simulator/
    ├── mod.rs                 # Deterministic simulator
    └── executor.rs            # Event-driven execution
```

## Dependencies

- `tokio` - Async runtime
- `parking_lot` - Efficient synchronization primitives
- `serde` / `serde_json` - Serialization for gossip protocol
- `rand` / `rand_chacha` - Deterministic RNG for simulator
- `tracing` - Structured logging

## License

MIT
