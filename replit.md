# Redis Deterministic Simulator

A deterministic simulator for building Redis from scratch in Rust, using FoundationDB's engineering approach to simulation testing.

## Overview

This project implements a **deterministic simulation framework** that allows testing distributed systems with perfect reproducibility. Following FoundationDB's pioneering approach, the simulator provides:

- **Single-threaded deterministic execution**: All events are processed in a controlled, reproducible order
- **Virtual time system**: Fast-forward through delays without actual waiting
- **Simulated network layer**: In-memory packet delivery with configurable faults
- **Seeded random number generation**: Same seed = same execution every time
- **BUGGIFY-style chaos injection**: Probabilistic fault injection for finding edge cases

## Recent Changes

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
- 15+ implemented commands
- Type checking (WRONGTYPE errors)
- Proper error handling

**Supported Commands:**
- Strings: GET, SET, DEL
- Lists: LPUSH, RPUSH, LPOP, RPOP, LRANGE
- Sets: SADD, SMEMBERS, SISMEMBER
- Hashes: HSET, HGET, HGETALL
- Sorted Sets: ZADD, ZRANGE, ZSCORE
- General: PING

#### Server (`server.rs`)
Redis server and client implementations for the simulator:
- `RedisServer`: Handles commands via simulated network
- `RedisClient`: Sends commands and receives responses

## Running the Simulator

```bash
# Build and run
cargo run --release

# Run tests
cargo test

# Build only
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
