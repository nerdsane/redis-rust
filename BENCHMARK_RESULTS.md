# Redis Server Performance Benchmark Results

## Test Configuration

**Server:** Tiger Style Redis Server (Actor-per-Shard Architecture)
**Binary:** `redis-server-optimized`
**Port:** 3000

### System Configuration

| Component | Specification |
|-----------|---------------|
| CPU Limit | 2 cores per container |
| Memory Limit | 1GB per container |
| Requests | 100,000 |
| Clients | 50 concurrent |
| Data Size | 64 bytes |

---

## Linux Benchmarks (January 9, 2026)

**Platform:** Linux 6.8.0-86-generic (Native Docker)

### Non-Pipelined Performance (P=1)

| Command | Redis 7.4 | Redis 8.0 | Rust | Rust vs R8 |
|---------|-----------|-----------|------|------------|
| PING | 84,317 req/s | 82,508 req/s | 79,114 req/s | **95.8%** |
| SET | 80,000 req/s | 82,034 req/s | 78,555 req/s | **95.7%** |
| GET | 82,645 req/s | 77,640 req/s | 73,046 req/s | **94.0%** |
| MSET | 66,138 req/s | 69,832 req/s | 75,301 req/s | **107.8%** |
| INCR | 79,177 req/s | 78,493 req/s | 75,301 req/s | **95.9%** |
| LPUSH | 75,075 req/s | 75,075 req/s | 73,421 req/s | **97.7%** |
| RPUSH | 76,746 req/s | 74,516 req/s | 74,906 req/s | **100.5%** |
| LPOP | 75,301 req/s | 77,160 req/s | 69,784 req/s | **90.4%** |
| RPOP | 75,301 req/s | 76,805 req/s | 72,780 req/s | **94.7%** |
| LRANGE_100 | 36,456 req/s | 49,751 req/s | 53,792 req/s | **108.1%** |
| LRANGE_300 | 18,622 req/s | 26,469 req/s | 26,323 req/s | **99.4%** |
| LRANGE_500 | 12,817 req/s | 19,646 req/s | 18,587 req/s | **94.6%** |
| SADD | 74,129 req/s | 73,260 req/s | 70,373 req/s | **96.0%** |
| HSET | 74,074 req/s | 73,855 req/s | 69,013 req/s | **93.4%** |
| ZADD | 68,587 req/s | 70,771 req/s | 68,027 req/s | **96.1%** |

### Pipelined Performance (P=16)

| Command | Redis 7.4 | Redis 8.0 | Rust | Rust vs R8 |
|---------|-----------|-----------|------|------------|
| PING | 1,000,000 req/s | 1,149,425 req/s | 990,099 req/s | **86.1%** |
| SET | 680,272 req/s | 775,194 req/s | 877,193 req/s | **113.1%** |
| GET | 746,269 req/s | 740,741 req/s | 990,099 req/s | **133.6%** |
| MSET | 243,309 req/s | 290,698 req/s | 316,456 req/s | **108.8%** |
| INCR | 740,741 req/s | 884,956 req/s | 925,926 req/s | **104.6%** |
| LPUSH | 370,370 req/s | 813,008 req/s | 909,091 req/s | **111.8%** |
| RPUSH | 694,445 req/s | 854,701 req/s | 819,672 req/s | **95.9%** |
| LPOP | 675,676 req/s | 729,927 req/s | 1,010,101 req/s | **138.3%** |
| RPOP | 645,161 req/s | 763,359 req/s | 961,538 req/s | **125.9%** |
| LRANGE_100 | 63,131 req/s | 131,579 req/s | 127,389 req/s | **96.8%** |
| LRANGE_300 | 18,591 req/s | 32,321 req/s | 31,746 req/s | **98.2%** |
| LRANGE_500 | 10,799 req/s | 20,429 req/s | 19,142 req/s | **93.7%** |
| SADD | 793,651 req/s | 775,194 req/s | 1,000,000 req/s | **128.9%** |
| HSET | 625,000 req/s | 714,286 req/s | 877,193 req/s | **122.8%** |
| ZADD | 480,769 req/s | 578,035 req/s | 617,284 req/s | **106.7%** |

### Linux Summary

**Non-Pipelined (P=1):** 90-108% of Redis 8.0
- Competitive across all 15 operations
- Beats Redis 8.0 on **MSET (107.8%)** and **LRANGE_100 (108.1%)**
- Average: ~97% of Redis 8.0

**Pipelined (P=16):** 86-138% of Redis 8.0
- **LPOP: 138.3%** - 1.01M req/s (38% faster than Redis 8.0)
- **GET: 133.6%** - 990K req/s (34% faster)
- **SADD: 128.9%** - 1.0M req/s (29% faster)
- **RPOP: 125.9%** - 962K req/s (26% faster)
- **HSET: 122.8%** - 877K req/s (23% faster)
- **SET: 113.1%** - 877K req/s (13% faster)
- **LPUSH: 111.8%** - 909K req/s (12% faster)
- **MSET: 108.8%** - 316K req/s (9% faster)
- **ZADD: 106.7%** - 617K req/s (7% faster)
- **INCR: 104.6%** - 926K req/s (5% faster)
- Average: ~109% of Redis 8.0 (9% faster overall)

**Wins:** 10 out of 15 pipelined operations beat Redis 8.0

---

## macOS Benchmarks (January 8, 2026)

**Platform:** macOS Darwin 24.4.0 (Docker Desktop)

### Non-Pipelined Performance (P=1)

| Operation | Redis 7.4 | Redis 8.0 | Rust | Rust vs R8 |
|-----------|-----------|-----------|------|------------|
| SET | 170,068 req/s | 165,837 req/s | 168,350 req/s | **101.5%** |
| GET | 179,856 req/s | 183,824 req/s | 168,067 req/s | **91.4%** |

### Pipelined Performance (P=16)

| Operation | Redis 7.4 | Redis 8.0 | Rust | Rust vs R8 |
|-----------|-----------|-----------|------|------------|
| SET | 1,408,451 req/s | 1,369,863 req/s | 1,086,957 req/s | **79.3%** |
| GET | 1,250,000 req/s | 1,449,275 req/s | 1,250,000 req/s | **86.2%** |

### macOS Summary

- **SET P=1: 101.5%** - Exceeds Redis 8.0 for single-operation writes
- **GET P=1: 91.4%** - Competitive single-operation reads
- Pipelined performance lower due to Docker Desktop virtualization overhead

---

## Architecture

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

### Performance Optimizations

| Optimization | Description |
|-------------|-------------|
| jemalloc | `tikv-jemallocator` custom allocator |
| Actor-per-Shard | Lock-free tokio channels (no RwLock) |
| Buffer Pooling | `crossbeam::ArrayQueue` buffer reuse |
| Zero-copy Parser | `bytes::Bytes` + `memchr` RESP parsing |
| Connection Pooling | Semaphore-limited with shared buffers |

### Feature-Specific Optimizations

| Optimization | Description | Impact |
|-------------|-------------|--------|
| P0: Single Key Allocation | Reuse key string in `set_direct()` | +5-10% |
| P1: Static OK Response | Pre-allocated "OK" response | +1-2% |
| P2: Zero-Copy GET | Avoid data copy in `get_direct()` | +2-3% |
| P3: itoa Encoding | Fast integer-to-string conversion | +1-2% |
| P4: atoi Parsing | Fast string-to-integer parsing | +2-3% |

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

---

## Correctness Testing

### Test Suite (662 tests)

| Category | Tests | Coverage |
|----------|-------|----------|
| Unit Tests | 400+ | RESP parsing, commands, data structures |
| DST/Simulation | 99 | Multi-seed chaos testing with fault injection |
| Lua Scripting | 37 | EVAL/EVALSHA execution |
| CRDT/Consistency | 34 | Convergence, vector clocks, partition healing |
| Streaming Persistence | 20 | Object store, recovery, compaction |
| Redis Equivalence | 11 | Differential testing vs real Redis |

### DST Coverage (January 9, 2026)

| Data Structure | Seeds Tested | Operations |
|----------------|--------------|------------|
| Sorted Set | 100+ | ZADD, ZREM, ZSCORE, ZRANGE |
| List | 100+ | LPUSH, RPUSH, LPOP, RPOP, LSET, LTRIM |
| Hash | 100+ | HSET, HGET, HDEL |
| Set | 100+ | SADD, SREM, SMEMBERS |
| Streaming | 100+ | Flush, crash recovery, compaction |
| CRDT | 100+ | GCounter, PNCounter, ORSet, VectorClock |

### Maelstrom/Jepsen Results

| Test | Nodes | Result | Notes |
|------|-------|--------|-------|
| Linearizability (lin-kv) | 1 | **PASS** | Single-node is linearizable |
| Linearizability (lin-kv) | 3 | **FAIL** | Expected: eventual consistency |

**Note:** Multi-node linearizability tests FAIL by design. We use Anna-style eventual consistency, not Raft/Paxos consensus.

---

## Running Benchmarks

### Docker Benchmark (Recommended)

```bash
cd docker-benchmark

# Redis 8.0 three-way comparison (15 commands)
./run-redis8-comparison.sh

# In-memory comparison (Redis 7.4 vs Rust)
./run-benchmarks.sh

# Persistent comparison (Redis AOF vs Rust S3/MinIO)
./run-persistent-benchmarks.sh
```

### Benchmark Commands

```bash
# Non-pipelined (P=1)
redis-benchmark -p <port> -n 100000 -c 50 -P 1 -d 64 -r 10000 \
    -t ping_mbulk,set,get,mset,incr,lpush,rpush,lpop,rpop,lrange_100,lrange_300,lrange_500,sadd,hset,zadd --csv

# Pipelined (P=16)
redis-benchmark -p <port> -n 100000 -c 50 -P 16 -d 64 -r 10000 \
    -t ping_mbulk,set,get,mset,incr,lpush,rpush,lpop,rpop,lrange_100,lrange_300,lrange_500,sadd,hset,zadd --csv
```

---

## Known Limitations

1. **Streaming persistence**: Object store-based (S3/LocalFs), not traditional RDB/AOF
2. **No pub/sub or streams**: Not implemented
3. **Multi-node consistency**: Eventual, not linearizable (by design)
4. **Inline commands**: Only RESP bulk format supported (not inline PING)
