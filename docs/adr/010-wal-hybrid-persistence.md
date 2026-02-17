# ADR-010: WAL + Streaming Hybrid Persistence

## Status

Accepted

## Context

The streaming persistence layer (ADR-005) streams deltas to object storage (S3/GCS/local) but there is a durability gap: the client receives a success response before the delta reaches the object store. If the server crashes between `execute()` and the next WriteBuffer flush (~250ms default), acknowledged writes are lost.

For use cases like session stores, job queues, and configuration stores, this durability gap is unacceptable. We need a way to guarantee that acknowledged writes survive crashes without abandoning the cloud-native streaming model.

## Decision

Add a local Write-Ahead Log (WAL) with group commit that closes the durability gap. The WAL provides microsecond-latency local durability; streaming provides cloud-scale long-term durability.

### Architecture

```
Client request -> ShardActor.execute(cmd) -> WalActor.write(delta) -> group commit -> fsync -> Response to client
                                                                                                |  (async)
                                                                  DeltaSink -> PersistenceActor -> ObjectStore
                                                                  |  (after successful stream)
                                                                  WalActor.truncate(streamed_timestamp)
```

The WAL sits between command execution and client acknowledgement. Streaming continues asynchronously in the background. Once a delta has been successfully streamed to the object store, the corresponding WAL entries are truncated.

### Fsync Policies (like Redis AOF)

| Policy | Behavior | RPO | Latency overhead |
|--------|----------|-----|------------------|
| Always | Group commit: batch + fsync before ack | 0 | ~2-10us/write (amortized) |
| EverySecond | Append + ack immediately, fsync every 1s | <=1s | ~0.1us/write |
| No | Append + ack immediately, OS decides fsync | Unbounded | ~0.1us/write |

Users choose their durability/performance tradeoff via configuration, matching the familiar Redis AOF model.

### Group Commit (turbopuffer-inspired)

The WAL actor runs a tight loop: accumulate entries from concurrent writers, issue one `fsync` for the batch, then resolve all waiters. This amortizes the cost of `fsync` across many writes. With 50 concurrent clients, a single `fsync` covers ~50 entries, reducing per-write overhead to ~2us.

### Recovery Order

1. Load from object store (checkpoint + segments) -- bulk state
2. Determine high-water mark (latest segment's `max_timestamp`)
3. Replay WAL entries with `timestamp > high_water_mark`
4. CRDT idempotency makes duplicate replay safe -- no dedup needed

Because all deltas are `ReplicationDelta` values containing CRDT operations, replaying an already-streamed entry is a no-op. This eliminates the need for complex exactly-once tracking during recovery.

### WAL File Format

```
Header (16 bytes):
  magic:    [u8; 4]  = "RWAL"
  version:  u8
  flags:    u8
  reserved: [u8; 2]
  sequence: u64 (LE)

Entry (variable length):
  data_length: u32 (LE)
  timestamp:   u64 (LE)
  checksum:    u32 (CRC32)
  data:        [u8; data_length]  (bincode-encoded ReplicationDelta)
```

CRC32 checksums are per-entry (not per-file) so that partial writes from crashes are detected at entry granularity. A torn write corrupts only the last entry, which is safely skipped during recovery.

## Consequences

### Positive

- **Zero-RPO durability** with `Always` fsync policy
- **Group commit** amortizes fsync cost (~2us/write with 50 concurrent clients)
- **Configurable tradeoff** between consistency and performance via familiar Redis AOF policies
- **CRDT-safe recovery** without dedup complexity -- replay is always idempotent
- **DST-verified** durability guarantees under simulated crashes and faults
- **Complements streaming** rather than replacing it -- WAL handles local durability, object store handles long-term cloud storage

### Negative

- **Additional local disk requirement** -- WAL directory must be on the same machine
- **Slight latency increase** for `Always` mode (amortized ~2-10us per write)
- **More complex recovery path** -- must reconcile object store state with WAL entries
- **WAL file management** -- rotation and truncation add operational complexity

### Risks

- **Local disk failure** -- WAL on a failed disk provides no durability; consider mirrored volumes for critical deployments
- **Recovery time** -- large WAL files (if streaming falls behind) could slow restart
- **Simulation fidelity** -- simulated fsync may not capture all real disk failure modes

### Use Cases

| Use Case | Configuration |
|----------|---------------|
| Session store (e-commerce) | fsync: always |
| Rate limiting / counters | fsync: everysec |
| Cache with persistence | fsync: no |
| Feature flags / config store | fsync: always |
| Job queue | fsync: always |

## Verification Pyramid

```
Layer 6: TLA+ (WalDurability.tla)              <- Protocol design proof
Layer 5: Stateright (persistence.rs extension)  <- Exhaustive state-space
Layer 4: DST (wal_dst.rs, 100+ seeds)          <- Implementation under random faults
Layer 3: Unit tests (wal.rs, wal_store.rs)      <- Component correctness
Layer 2: Integration (recovery + pipeline)      <- End-to-end correctness
Layer 1: Manual (kill -9 + restart)             <- Smoke test
```

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-02-16 | Initial ADR created | Close durability gap between execute() and object store flush |
| 2026-02-16 | Use CRC32 per entry (not per file) | Crash tolerance: detect partial writes at entry granularity |
| 2026-02-16 | Group commit batch size 64 entries | Balance between latency and throughput based on turbopuffer pattern |
| 2026-02-16 | WAL rotation at 64MB | Match segment target size for predictable resource usage |
| 2026-02-16 | CRDT idempotency eliminates dedup | Simplifies recovery significantly vs exactly-once approaches |

## Implementation Status

### Implemented

| Component | Location | Status |
|-----------|----------|--------|
| WAL store trait + impls | `wal_store.rs` | Local filesystem + simulated implementations |
| WAL format, writer, reader | `wal.rs` | Header/entry format with CRC32 checksums |
| WAL configuration types | `wal_config.rs` | Fsync policies, rotation settings |
| SimulatedWalStore | `wal_store.rs` | Fault-injectable WAL for DST |
| WAL actor (group commit) | `wal_actor.rs` | Batch + fsync loop with waiter resolution |
| WAL DST harness | `wal_dst.rs` | Crash/fault simulation, 100+ seeds |
| Recovery integration | Recovery path | Object store + WAL replay with high-water mark |
| Production wiring | Server startup | WAL actor integrated into shard pipeline |

### Validated

- Group commit amortizes fsync across concurrent writers
- CRC32 detects torn writes from simulated crashes
- Recovery replays only entries above object store high-water mark
- CRDT idempotency makes duplicate replay safe
- DST with 100+ seeds and fault injection passes

### Formal Verification

| Component | Location | Status |
|-----------|----------|--------|
| WalDurability.tla | `specs/tla/WalDurability.tla` | 5 invariants + 3 temporal properties |
| Stateright WalDurabilityModel | `src/stateright/persistence.rs` | 5 properties, exhaustive BFS |
| Multi-node integration | `docker/docker-compose.wal-integration.yml` | 6 test scenarios |

## References

- [turbopuffer queue.json](https://turbopuffer.com/blog/object-storage-queue) -- group commit pattern
- [Redis AOF](https://redis.io/docs/management/persistence/#append-only-file) -- fsync policies
- [FoundationDB simulation](https://apple.github.io/foundationdb/testing.html) -- DST methodology
- [ADR-005: Streaming Persistence](./005-streaming-persistence.md) -- the streaming layer this WAL complements
- [ADR-001: Simulation-First Development](./001-simulation-first-development.md) -- DST methodology this WAL follows
