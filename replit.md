# Redis Cache - Production Server + Deterministic Simulator

## Overview

This project implements a production-ready, actor-based Redis cache server and a deterministic simulator for robust testing. It allows running Redis in two modes: as a high-performance, sharded production server compatible with `redis-cli`, and as a single-threaded, deterministic simulator for comprehensive testing of distributed scenarios, inspired by FoundationDB's engineering approach. The core caching logic is shared between both implementations, ensuring that the code tested in simulation is the same code deployed in production. The project aims to provide a reliable, performant, and thoroughly testable in-memory data store solution.

## User Preferences

I prefer concise and clear explanations.
I value an iterative development approach.
Please ask for my confirmation before making significant architectural changes or adding new external dependencies.
Focus on practical, implementable solutions rather than purely theoretical discussions.
I prefer to maintain a consistent coding style throughout the project.
Do not make changes to the folder `Z`
Do not make changes to the file `Y`

## System Architecture

The project is divided into two main components: a production Redis server and a deterministic simulator, both sharing core Redis logic.

### UI/UX Decisions
N/A (Backend project)

### Technical Implementations

- **Production Server** (Tiger Style, `redis-server-optimized`):
    - **Tiger Style Principles**: Explicit over implicit, debug_assert! invariants, no silent failures, deterministic behavior
    - **Actor-Based Architecture**: Tokio actors with lock-free message passing (ShardMessage enum)
    - **Sharding**: 16-shard keyspace with actor-per-shard for lock-free parallel execution
    - **TTL Manager Actor**: Background TtlManagerActor with explicit EvictExpired messages
    - **RESP Protocol**: Zero-copy parser with explicit error handling (protocol errors drain buffer)
    - **Command Set**: 35+ Redis commands with explicit error responses
- **Performance Optimizations** (`redis-server-optimized`):
    - **jemalloc Allocator**: Custom memory allocator for reduced fragmentation (~10% improvement)
    - **Actor-per-Shard**: Lock-free message passing replaces RwLock (~30% improvement)
    - **Buffer Pooling**: `crossbeam::ArrayQueue` for buffer reuse (~20% improvement)
    - **Zero-copy RESP Parser**: `bytes::Bytes` + `memchr` for efficient parsing (~15% improvement)
    - **Connection Pooling**: Semaphore-limited connections with shared buffer pools (~10% improvement)
- **Deterministic Simulator**:
    - **Single-threaded Execution**: Guarantees reproducibility by processing all events in a controlled, predetermined order.
    - **Virtual Time System**: Allows fast-forwarding through delays for rapid testing of long-duration scenarios.
    - **Simulated Network Layer**: In-memory packet delivery with configurable fault injection (latency, drops, partitions).
    - **Seeded Random Number Generation**: Uses ChaCha8 PRNG with fixed seeds to ensure identical execution for the same input.
    - **BUGGIFY-style Chaos Injection**: Probabilistic fault injection to uncover rare edge cases.
    - **Core Logic Reusability**: The `CommandExecutor` and data structures are shared directly with the production server.

### Feature Specifications

- **Redis Commands**: Implements a comprehensive set of Redis commands including `GET`, `SET`, `SETEX`, `EXPIRE`, `INCR`, `DECR`, `LPUSH`, `RPUSH`, `SADD`, `HSET`, `ZADD`, `EXISTS`, `TYPE`, `KEYS`, `FLUSHDB`, `FLUSHALL`, `PING`, `INFO`, and more.
- **Data Structures**: Binary-safe SDS (Simple Dynamic String), `VecDeque`-based Lists, `HashSet`-based Sets, `HashMap`-based Hashes, and a Skip List-based Sorted Set.
- **Expiration**: Full support for `TTL`, `PTTL`, `EXPIRE`, `EXPIREAT`, `PEXPIREAT`, and `PERSIST` commands, with `EXPIREAT`/`PEXPIREAT` handling Unix epoch timestamps relative to a configurable `simulation_start_epoch`.
- **Performance**: The optimized production server achieves approximately 40,000 operations/second with sub-millisecond latency.

### System Design Choices

- **FoundationDB Philosophy**: Emphasizes testing production code within a deterministic simulator for high confidence.
- **Event-driven Architecture**: Core of the simulator, managing all actions as events in a priority queue.
- **Network Shims**: Replaces real network I/O with in-memory simulation for determinism.
- **Data Structure Fidelity**: Redis data structures are implemented to match real Redis semantics.

## External Dependencies

- `rand`: For general random number generation trait implementations.
- `rand_chacha`: Specifically for the ChaCha8 pseudo-random number generator, crucial for deterministic simulations.
- `fnv`: Provides a fast hash function used for internal hash tables.
- `tokio`: Asynchronous runtime for the production server's actor-based concurrency.
- `parking_lot`: Provides efficient synchronization primitives, specifically `RwLock`, for thread-safe state management in the production server.
- `serde` / `serde_json`: JSON serialization for Maelstrom protocol support.

## Anna KVS-Style Replication

The project includes Anna KVS-inspired replication with configurable consistency:

- **Lattice-based CRDTs**: LWW (Last-Writer-Wins) registers with Lamport clocks for conflict resolution
- **Vector clocks**: For causal consistency tracking (optional)
- **Gossip protocol**: Periodic state synchronization between nodes
- **Configurable consistency levels**: Eventual (default) or Causal
- **Sharded replication**: Each of 16 shards maintains independent replica state

Key files:
- `src/replication/lattice.rs` - CRDT primitives (LwwRegister, VectorClock, LamportClock)
- `src/replication/state.rs` - Shard replica state management
- `src/replication/gossip.rs` - Gossip message types
- `src/production/replicated_state.rs` - Replicated sharded state
- `src/production/gossip_manager.rs` - Peer-to-peer gossip networking

## Correctness Testing (Maelstrom/Jepsen)

The project includes Maelstrom integration for formal linearizability testing:

- **maelstrom-kv binary**: Single-node, speaks Maelstrom's JSON protocol, translates to Redis commands
- **maelstrom-kv-replicated binary**: Multi-node with gossip-based replication between nodes
- **lin-kv workload**: Tests read/write/compare-and-swap operations for linearizability
- **Single-node tests**: Verify core Redis implementation is linearizable
- **Multi-node tests**: Verify replicated state convergence with eventual consistency

Run tests with: `./scripts/maelstrom_test.sh`

Test results (verified):
- Single-node linearizability: PASS
- Multi-node (3 nodes) with replication: PASS

## Recent Changes (January 2026)

### Tiger Style Cleanup
- Removed legacy files: `shared_state.rs`, `server.rs`, `connection.rs`, `sharded_state.rs`
- Unified on optimized actor-based components
- Added `ShardMessage` enum with explicit `Command` and `EvictExpired` variants
- Implemented `TtlManagerActor` with actor-compatible TTL eviction
- Added `debug_assert!` invariants for shard bounds, channel failures, buffer capacity
- Explicit error handling: parse errors drain buffer and return protocol error
- Buffer overflow protection with 1MB limit
- All 14 unit tests pass; redis-cli integration verified

Key production files:
- `src/production/server_optimized.rs` - Main server with TTL manager spawn
- `src/production/sharded_actor.rs` - Actor-per-shard with ShardMessage enum
- `src/production/connection_optimized.rs` - Zero-copy connection handler
- `src/production/ttl_manager.rs` - TtlManagerActor for key expiration

### Deterministic Simulation Harness (FDB/TigerBeetle Style)
- `SimulationHarness` - Wraps CommandExecutor with VirtualTime for deterministic testing
- `SimulatedRedisNode` - Single Redis node with explicit time control
- `ScenarioBuilder` - Ergonomic test authoring with time-based scheduling
- `run_with_eviction()` - Interleaves TTL eviction cycles for expiration testing
- History recording for linearizability verification

Simulation tests (8 tests):
- `test_basic_set_get` - Baseline operations
- `test_ttl_expiration_with_fast_forward` - Virtual time fast-forward
- `test_ttl_boundary_race` - Expiration edge cases at exact boundary
- `test_concurrent_increments` - Multiple clients at same timestamp
- `test_deterministic_replay` - Same seed produces identical results
- `test_buggify_chaos` - Probabilistic delay injection
- `test_persist_cancels_expiration` - PERSIST command behavior
- `test_multi_seed_invariants` - 100 seeds verify invariants hold

Key simulation files:
- `src/simulator/harness.rs` - SimulationHarness, ScenarioBuilder, tests
- `src/simulator/rng.rs` - ChaCha8-based DeterministicRng, buggify()
- `src/simulator/time.rs` - VirtualTime with deterministic ordering