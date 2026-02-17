# ADR-002: Actor-per-Shard Architecture

## Status

Accepted

## Context

Redis achieves high performance through a single-threaded event loop that avoids lock contention. However, this limits scalability to a single CPU core. Multi-threaded Redis alternatives typically use:

1. **Shared state with locks**: Simple but creates contention under load
2. **Partitioned state with routing**: Complex but scales linearly

Our benchmarks showed that `RwLock<HashMap>` implementations plateau at ~150K req/s due to lock contention, while message-passing architectures can exceed 1M req/s with pipelining.

We need an architecture that:
- Scales across multiple CPU cores
- Avoids lock contention in the hot path
- Maintains isolation between shards for fault tolerance
- Supports graceful shutdown and dynamic reconfiguration

## Decision

We will use an **Actor-per-Shard Architecture** where:

1. **Each shard is an independent actor** with its own state and message queue
2. **Actors communicate via tokio::mpsc channels** (lock-free, bounded)
3. **Key routing uses consistent hashing** to map keys to shards
4. **No shared mutable state** between shards
5. **Dynamic shard count** configurable at runtime (default: num_cpus)

### Architecture

```
Client Connections
        |
   [Tokio Runtime]
        |
   [Connection Handler]
        |
   hash(key) % num_shards
        |
   [ShardActor 0..N]  <-- tokio::mpsc channels (lock-free)
        |
   [CommandExecutor per shard]
```

### Message Types

```rust
pub enum ShardMessage {
    // Core command execution
    Command { cmd, virtual_time, response_tx },
    BatchCommand { cmd, virtual_time },         // Fire-and-forget
    EvictExpired { virtual_time, response_tx },

    // Fast-path optimizations (bypass Command enum)
    FastGet { key, response_tx },
    FastSet { key, value, response_tx },
    FastBatchGet { keys, response_tx },
    FastBatchSet { pairs, response_tx },

    // Response-pooled variants (reduce allocations)
    PooledFastGet { key, slot },
    PooledFastSet { key, value, slot },
}
// Note: Shutdown is achieved by dropping the channel sender,
// not via a Shutdown message variant.
```

### Actor Pattern

```rust
impl ShardActor {
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ShardMessage::Command { cmd, response_tx } => {
                    let result = self.executor.execute(cmd);
                    let _ = response_tx.send(result);
                }
                ShardMessage::EvictExpired => {
                    self.executor.evict_expired();
                }
                ShardMessage::Shutdown { response } => {
                    let _ = response.send(());
                    break;
                }
            }
        }
    }
}
```

## Consequences

### Positive

- **Linear scalability**: Each shard runs on its own task, utilizing multiple cores
- **No lock contention**: Message passing eliminates RwLock hotspots
- **Isolation**: Shard failures don't affect other shards
- **Graceful shutdown**: Shutdown messages ensure clean state
- **Testability**: Actors can be tested in isolation
- **~30% performance improvement**: Over RwLock-based design in benchmarks

### Negative

- **Routing overhead**: Every command requires hashing and channel send
- **Cross-shard operations**: MGET/MSET require scatter-gather
- **Memory overhead**: Each shard has its own data structures
- **Complexity**: More moving parts than single-threaded design

### Risks

- **Channel backpressure**: Slow shards can cause upstream blocking
- **Load imbalance**: Zipfian workloads may overload specific shards
- **Debugging difficulty**: Distributed state harder to inspect

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-03 | Initial ADR created | Actor model chosen over RwLock for performance |
| 2026-01-03 | Use tokio::mpsc for channels | Native tokio integration, bounded queues |
| 2026-01-04 | Default shard count = num_cpus | Optimal for CPU-bound workloads |
| 2026-01-04 | Add TTL manager as separate actor | Decouples expiration from command processing |
| 2026-01-05 | Use oneshot for responses | Clean request-response pattern, no allocation reuse |
| 2026-01-06 | Add ShardConfig for tuning | Allow runtime configuration of channel sizes |
| 2026-01-07 | Implement ReplicatedShardActor | Extend actor pattern for CRDT replication |

## Implementation Status

### Implemented

| Component | Location | Status |
|-----------|----------|--------|
| ShardedActorState | `src/production/sharded_actor.rs` | Core sharding with actor model |
| ShardActor | `src/production/sharded_actor.rs` | Per-shard actor implementation |
| ShardConfig | `src/production/sharded_actor.rs` | Configuration for shard count, channel size |
| TtlManagerActor | `src/production/ttl_manager.rs` | Background TTL eviction actor |
| ReplicatedShardActor | `src/production/replicated_shard_actor.rs` | Actor with CRDT replication |
| GossipActor | `src/production/gossip_actor.rs` | Actor for gossip protocol |
| AdaptiveActor | `src/production/adaptive_actor.rs` | Actor with adaptive replication |
| OptimizedRedisServer | `src/production/server_optimized.rs` | Production server using actors |

### Validated

- Actor model achieves ~1M req/s with pipelining (P=16)
- Lock-free design eliminates contention under load
- Graceful shutdown completes without data loss
- TTL manager evicts expired keys correctly

### Not Yet Implemented

| Component | Notes |
|-----------|-------|
| Dynamic shard rebalancing | `src/production/load_balancer.rs` provides metrics and `ScalingDecision` data structures, but not yet integrated into shard actor lifecycle |
| Actor supervision | No automatic restart on failure |
| Work stealing | No load balancing between shards |

## References

- [Tokio: Message Passing](https://tokio.rs/tokio/tutorial/channels)
- [Actor Model](https://en.wikipedia.org/wiki/Actor_model)
- [Redis Cluster Specification](https://redis.io/docs/reference/cluster-spec/)
- [DragonFly Architecture](https://github.com/dragonflydb/dragonfly/blob/main/docs/dashtable.md)
