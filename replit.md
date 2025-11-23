# Redis Cache - Production Server + Deterministic Simulator

A production-ready Redis cache server with an actor-based architecture, alongside a deterministic simulator for testing using FoundationDB's engineering approach.

## Overview

This project provides **two ways to run Redis**:

### 1. Production Server (Actor-Based, Real I/O)
- ✅ **TCP server** on port 3000 (compatible with redis-cli)
- ✅ **Actor-based architecture** using Tokio for concurrent connections
- ✅ **Real-time TTL expiration** with background actor
- ✅ **35+ Redis commands** for production caching workloads
- ✅ **Thread-safe shared state** with parking_lot RwLock
- ✅ **Production-ready** for real applications

### 2. Deterministic Simulator (Testing Harness)
- **Single-threaded deterministic execution**: All events are processed in a controlled, reproducible order
- **Virtual time system**: Fast-forward through delays without actual waiting
- **Simulated network layer**: In-memory packet delivery with configurable faults
- **Seeded random number generation**: Same seed = same execution every time
- **BUGGIFY-style chaos injection**: Probabilistic fault injection for finding edge cases

**Both implementations share the same core caching logic** (CommandExecutor, data structures, RESP protocol), following the FoundationDB philosophy: test production code in a deterministic simulator.

## Recent Changes

**November 23, 2025**: Production Redis Server Added
- ✅ **Actor-based production server** running on port 3000
- ✅ **Tokio async runtime** for high-performance concurrent connections
- ✅ **Background TTL expiration actor** with real-time cleanup
- ✅ **Shared state with parking_lot RwLock** for thread-safe access
- ✅ **Full RESP protocol** over TCP sockets
- ✅ **Test client** verifying all 35+ commands work correctly
- **Architecture**: Production server reuses ALL simulator logic (CommandExecutor, data structures, RESP parser)

**November 23, 2025**: Full caching features added
- ✅ **TTL & Expiration**: SETEX, EXPIRE, EXPIREAT, TTL, PTTL, PERSIST commands
- ✅ **Atomic Counters**: INCR, DECR, INCRBY, DECRBY for cache counters
- ✅ **Advanced String Ops**: APPEND, GETSET, SETNX, MGET, MSET for batch operations
- ✅ **Key Management**: EXISTS, TYPE, KEYS, FLUSHDB, FLUSHALL commands
- ✅ **Server Stats**: INFO command for monitoring cache statistics
- ✅ **Automatic Expiration**: Background key eviction based on virtual time
- ✅ **Access Tracking**: LRU-ready access time tracking for all keys
- **Total**: 35+ Redis commands implemented for production caching

**November 22, 2025**: Initial implementation
- Built complete simulation framework with virtual time, deterministic RNG, and network simulation
- Implemented Redis core data structures: SDS, Lists, Sets, Hashes, Sorted Sets
- Created RESP protocol parser and command execution engine
- Added 15+ Redis commands (GET/SET, LPUSH/RPOP, SADD/SMEMBERS, HSET/HGET, ZADD/ZRANGE, etc.)
- Demonstrated deterministic replay and fault injection capabilities
- All tests passing successfully

## Project Architecture

### Simulation Framework (`src/simulator/`)

#### Executor (`executor.rs`)
The core simulation engine that manages:
- Event priority queue (based on virtual time)
- Host management
- Timer scheduling
- Message delivery coordination

**Key API:**
```rust
let config = SimulationConfig {
    seed: 42,
    max_time: VirtualTime::from_secs(60),
};
let mut sim = Simulation::new(config);

// Add hosts
let server = sim.add_host("server".to_string());
let client = sim.add_host("client".to_string());

// Schedule events
sim.schedule_timer(host_id, Duration::from_millis(100));
sim.send_message(from, to, payload);

// Run simulation
sim.run(|sim, event| {
    // Handle events
});
```

#### Virtual Time (`time.rs`)
Simulated time that can be fast-forwarded without waiting:
- `VirtualTime`: Represents a point in simulated time (milliseconds)
- `Duration`: Time intervals for scheduling

#### Network Layer (`network.rs`)
Simulates network communication with fault injection:
- Configurable latency ranges
- Packet drop rates
- Network partitions between hosts
- All network I/O is deterministic

**Fault Injection:**
```rust
sim.set_network_drop_rate(0.2);  // 20% packet loss
sim.partition_hosts(host1, host2);  // Network partition
```

#### Deterministic RNG (`rng.rs`)
ChaCha8-based PRNG with fixed seeds:
- `DeterministicRng::new(seed)`: Create seeded RNG
- `buggify()`: FoundationDB-style chaos macro (1% probability)

### Redis Implementation (`src/redis/`)

#### Data Structures (`data.rs`)

**SDS (Simple Dynamic String)**
```rust
pub struct SDS {
    data: Vec<u8>,
}
```
Binary-safe string with O(1) length operations, similar to Redis's implementation.

**RedisList**
VecDeque-based list supporting:
- LPUSH/RPUSH: O(1) insertion at both ends
- LPOP/RPOP: O(1) removal from both ends
- LRANGE: Range queries with negative indexing

**RedisSet**
HashSet-based unordered collection:
- SADD: O(1) insertion
- SMEMBERS: O(N) get all members
- SISMEMBER: O(1) membership test

**RedisHash**
HashMap for field-value pairs:
- HSET/HGET: O(1) operations
- HGETALL: O(N) get all fields

**RedisSortedSet**
Skip list implementation with score-based ordering:
- ZADD: Add members with scores
- ZRANGE: Range queries by rank
- ZSCORE: Get member's score

#### RESP Protocol (`resp.rs`)
Complete Redis Serialization Protocol parser:
- Parses: SimpleStrings, Errors, Integers, BulkStrings, Arrays
- Encodes responses back to RESP format
- Handles null values and nested arrays

#### Commands (`commands.rs`)
Command parsing and execution:
- 35+ implemented commands for full caching support
- Type checking (WRONGTYPE errors)
- Proper error handling
- TTL/Expiration support

**Supported Commands:**

**String Operations (Caching Core):**
- `GET`, `SET`, `DEL` - Basic key-value operations
- `SETEX` - Set with expiration (TTL in seconds)
- `SETNX` - Set if not exists (atomic cache lock)
- `APPEND` - Append to string value
- `GETSET` - Atomic get-and-set
- `MGET`, `MSET` - Batch get/set multiple keys

**Atomic Counters:**
- `INCR`, `DECR` - Increment/decrement by 1
- `INCRBY`, `DECRBY` - Increment/decrement by N

**Expiration & TTL:**
- `EXPIRE` - Set TTL in seconds (supports immediate expiration with TTL <= 0)
- `EXPIREAT` - Set expiration at Unix timestamp (seconds since epoch)
- `PEXPIREAT` - Set expiration at Unix timestamp (milliseconds since epoch)
- `TTL` - Get remaining TTL in seconds
- `PTTL` - Get remaining TTL in milliseconds
- `PERSIST` - Remove expiration from key

**EXPIREAT/PEXPIREAT Implementation**: The simulator supports Redis-compatible Unix epoch timestamps through a configurable `simulation_start_epoch` anchor point. When EXPIREAT receives a Unix timestamp in seconds (or PEXPIREAT in milliseconds), it converts it to simulation-relative time by subtracting the `simulation_start_epoch`, allowing proper handling of real-world expiration times in the deterministic virtual time system.

**Key Management:**
- `EXISTS` - Check if keys exist
- `TYPE` - Get value type (string/list/set/hash/zset)
- `KEYS` - Find keys matching pattern
- `FLUSHDB`, `FLUSHALL` - Clear all data

**Lists:**
- `LPUSH`, `RPUSH` - Push to left/right
- `LPOP`, `RPOP` - Pop from left/right
- `LRANGE` - Get range of elements

**Sets:**
- `SADD` - Add members
- `SMEMBERS` - Get all members
- `SISMEMBER` - Check membership

**Hashes:**
- `HSET`, `HGET` - Set/get hash field
- `HGETALL` - Get all fields

**Sorted Sets:**
- `ZADD` - Add with score
- `ZRANGE` - Get range by rank
- `ZSCORE` - Get member's score

**Server:**
- `PING` - Health check
- `INFO` - Server statistics and metrics

#### Server (`server.rs`)
Redis server and client implementations for the simulator:
- `RedisServer`: Handles commands via simulated network
- `RedisClient`: Sends commands and receives responses

## Running the Project

### Production Redis Server

```bash
# Run the production server (port 3000)
cargo run --bin redis-server --release

# Connect with redis-cli (or any Redis client)
redis-cli -h localhost -p 3000

# Or use the test client
cargo run --bin test-client --release
```

### Deterministic Simulator

```bash
# Run the simulator with test scenarios
cargo run --bin redis-sim --release

# Run unit tests
cargo test

# Build both
cargo build --release
```

## Example Usage

### Basic Deterministic Test

```rust
use redis_sim::{Simulation, SimulationConfig, RedisServer, RedisClient};
use redis_sim::simulator::VirtualTime;

let config = SimulationConfig {
    seed: 42,  // Same seed = same results
    max_time: VirtualTime::from_secs(10),
};

let mut sim = Simulation::new(config);
let server_host = sim.add_host("server".to_string());
let client_host = sim.add_host("client".to_string());

let mut server = RedisServer::new(server_host);
let mut client = RedisClient::new(client_host, server_host);

// Send Redis commands
let set_cmd = b"*3\r\n$3\r\nSET\r\n$3\r\nkey\r\n$5\r\nvalue\r\n".to_vec();
client.send_command(&mut sim, set_cmd);

// Run simulation
sim.run(|sim, event| {
    server.handle_event(sim, event);
    client.handle_event(event);
});
```

### Deterministic Replay

```rust
let seed = 12345;

// First run
let results1 = run_simulation_with_seed(seed);

// Second run - identical results!
let results2 = run_simulation_with_seed(seed);

assert_eq!(results1, results2);
```

### Chaos Testing with BUGGIFY

```rust
use redis_sim::simulator::buggify;

sim.run(|sim, event| {
    // Inject rare delays/faults
    if buggify(sim.rng()) {
        // Simulate slow disk, network delay, etc.
        sim.schedule_timer(host, Duration::from_secs(10));
    }
    
    // Normal processing
    handle_event(event);
});
```

### Network Fault Injection

```rust
// Simulate 20% packet loss
sim.set_network_drop_rate(0.2);

// Simulate network partition
sim.partition_hosts(datacenter1, datacenter2);

// Later: heal the partition
sim.heal_partition(datacenter1, datacenter2);
```

## Key Concepts from FoundationDB

### 1. Deterministic Execution
- All randomness comes from seeded PRNG
- All events processed in time order
- No real threading or async - single-threaded simulation
- Perfect reproducibility for debugging

### 2. Simulation Testing
- Test distributed scenarios without real clusters
- Time compression: years of testing in minutes
- Covers edge cases impossible in real-world testing
- Same code runs in simulation and production (with shims)

### 3. BUGGIFY
Probabilistic fault injection inspired by FoundationDB:
```rust
if buggify(sim.rng()) {
    // This happens ~1% of the time
    // Inject delays, drops, or interesting conditions
}
```

This biases the simulator toward rare, dangerous conditions that find deep bugs.

## Testing Capabilities

The simulator demonstrates:
1. ✅ **Deterministic replay**: Same seed produces identical results
2. ✅ **Network faults**: Packet loss, delays, partitions
3. ✅ **Chaos injection**: BUGGIFY-style probabilistic faults
4. ✅ **Time compression**: Fast-forward through simulated time
5. ✅ **Basic Redis operations**: All core data types working

## Project Structure

```
redis-sim/
├── src/
│   ├── lib.rs              # Library entry point
│   ├── main.rs             # Demo program
│   ├── simulator/          # Simulation framework
│   │   ├── mod.rs         # Module exports
│   │   ├── executor.rs    # Event loop and scheduling
│   │   ├── time.rs        # Virtual time system
│   │   ├── network.rs     # Simulated network layer
│   │   └── rng.rs         # Deterministic random numbers
│   └── redis/              # Redis implementation
│       ├── mod.rs         # Module exports
│       ├── data.rs        # Data structures (SDS, List, Set, Hash, ZSet)
│       ├── resp.rs        # RESP protocol parser
│       ├── commands.rs    # Command parsing and execution
│       └── server.rs      # Server and client for simulator
├── Cargo.toml              # Dependencies and project config
└── replit.md               # This file

```

## Dependencies

- `rand`: Random number generation (trait implementations)
- `rand_chacha`: ChaCha8 PRNG for determinism
- `fnv`: Fast hash function for internal hash tables

## Design Decisions

1. **Single-threaded execution**: Eliminates non-determinism from race conditions
2. **Virtual time**: Allows fast testing without real waiting
3. **Seeded RNG**: ChaCha8 provides cryptographically strong determinism
4. **Event-driven architecture**: All actions modeled as events in priority queue
5. **Network shims**: Replace real sockets with in-memory simulation
6. **Data structure fidelity**: Redis structures match real Redis semantics

## Future Enhancements

Potential additions (not yet implemented):
- Persistence (RDB/AOF simulation)
- Replication and clustering
- More Redis commands (EXPIRE, transactions, pub/sub)
- Metrics and visualization
- Automated workload generators
- Performance regression testing
- Multi-datacenter topologies

## Acknowledgments

This project is inspired by:
- **FoundationDB**: Pioneering deterministic simulation testing
- **Redis**: One of the most elegant in-memory data structures
- **Turmoil/Madsim**: Rust deterministic simulation frameworks
- **Flow**: FoundationDB's actor-based concurrency model

## License

This is an educational project demonstrating deterministic simulation techniques.
