---
name: actor-model
description: Domain knowledge for the actor architecture, message passing, and connection handling
user_invocable: true
---

# Actor Model — redis-rust

You are about to work on actor-based code. This skill injects the concepts and codebase
mappings you need. Every file path, type name, and function signature below is drawn from
the actual source.

---

## 1. Actor Fundamentals (General Theory)

The actor model (Hewitt, 1973) is a mathematical model of concurrent computation where
the **actor** is the universal primitive. These properties are definitional — not
project-specific:

- **Encapsulated state.** An actor's state is private. No other actor can read or write
  it. The only way to affect an actor's state is to send it a message.
- **Message passing.** Actors communicate exclusively by sending asynchronous messages.
  Messages are buffered in a mailbox. There is no shared memory between actors.
- **Sequential processing.** An actor processes one message at a time. This eliminates
  data races within an actor by construction — no locks needed.
- **Supervision.** Actors can create (spawn) other actors and supervise their lifecycle.
  A parent actor can restart or stop child actors on failure.

**Why actors for a database server?** Redis is single-threaded. To scale beyond one core
while preserving the "sequential commands per key" guarantee, we shard keys across actors.
Each shard-actor processes commands sequentially (like Redis), while multiple shard-actors
run concurrently across cores. This is the actor-per-shard architecture.

**Concurrency vs parallelism:** Within one actor = sequential (no concurrency). Across
actors = parallel (true concurrency). The message-passing boundary is where concurrency
happens.

---

## 2. Our Actor Hierarchy (Project-Specific)

```
ConnectionHandler (per TCP connection)
    |
    v  (ShardMessage via mpsc)
ShardedActor (routes commands to correct shard)
    |
    v  (ShardMessage via mpsc)
Shard executors (1 per shard, each owns a CommandExecutor)
    |
    v  (replication deltas)
ReplicatedShardActor (wraps executor with replication)
    |
    v  (gossip messages)
GossipActor (disseminates to peers)
```

### Key Files

| File | Actor | Purpose |
|------|-------|---------|
| `src/production/connection_optimized.rs` | ConnectionHandler | TCP connection, RESP parsing, MULTI/EXEC state |
| `src/production/sharded_actor.rs` | ShardedActor | Key-based routing, fan-out commands |
| `src/redis/executor/mod.rs` | CommandExecutor | Per-shard command execution (the state) |
| `src/production/replicated_shard_actor.rs` | ReplicatedShardActor | Replication delta generation |
| `src/production/gossip_actor.rs` | GossipActor | Gossip protocol dissemination |
| `src/production/adaptive_actor.rs` | AdaptiveActor | Dynamic shard scaling |

---

## 3. ShardedActor — The Router

**File:** `src/production/sharded_actor.rs`

### Configuration

```rust
pub struct ShardConfig {
    pub initial_shards: usize,        // Default: num_cpus
    pub min_shards: usize,            // Default: 1
    pub max_shards: usize,            // Default: 256
    pub auto_scale: bool,             // Default: false
    pub adaptive_replication: bool,   // Default: false
    pub load_check_interval_ms: u64,  // Default: 10000
}
```

### Message Types

```rust
pub enum ShardMessage {
    Command { cmd, virtual_time, response_tx },
    BatchCommand { cmd, virtual_time },
    EvictExpired { virtual_time, response_tx },
    FastGet { key, response_tx },
    FastSet { key, value, response_tx },
    // ... more variants
}
```

### Routing

Commands are routed to shards by hashing the primary key:

```rust
let shard_index = hash(key) % num_shards;
```

Uses AHash by default, or FxHash with `#[cfg(feature = "opt-fxhash-routing")]`.

### Fan-out Commands

These commands need to see ALL shards and are handled specially in `sharded_actor.rs`:

| Command | Fan-out behavior |
|---------|-----------------|
| `DBSIZE` | Sum counts from all shards |
| `SCAN` | Merge results from all shards |
| `KEYS` | Collect from all shards |
| `EXISTS` (multi-key) | Check across shards |
| `DEL` (multi-key) | Delete across shards |
| `FLUSHDB` / `FLUSHALL` | Send to all shards |

If you add a new command that needs all-shard visibility, add aggregation logic in
`sharded_actor.rs`.

---

## 4. Connection Handler — MULTI/EXEC State Machine

**File:** `src/production/connection_optimized.rs`

The connection handler is an actor per TCP connection. It manages:

### Transaction State (connection-level, NOT per-shard)

```
Normal Mode ──MULTI──> Transaction Mode ──EXEC──> Normal Mode
     ^                       |                        |
     |                   DISCARD                      |
     └───────────────────────┘                        |
     ^                                                |
     └────────────────────────────────────────────────┘
```

- **MULTI**: Enter transaction mode. Subsequent commands are queued (not executed).
- **EXEC**: Execute all queued commands atomically on the shard.
- **DISCARD**: Abort transaction, clear queue, return to normal.
- **WATCH**: Snapshot key values at WATCH time. At EXEC time, re-read and compare.
  If any watched key changed, EXEC returns nil (transaction aborted).

**Critical:** WATCH inside MULTI is an error (returns immediately, not queued).
This was a bug we fixed — see `transaction_dst.rs` for the test.

### Commands That Bypass Transaction Queuing

These execute immediately even inside MULTI:
- `EXEC`, `DISCARD`, `MULTI` (errors if already in MULTI)
- `WATCH` (errors if inside MULTI)

### Fast Path

`connection_optimized.rs` has a fast path for simple GET/SET that bypasses full command
parsing when possible (`FastGet`, `FastSet` messages).

---

## 5. CommandExecutor — Per-Shard State

**File:** `src/redis/executor/mod.rs`

```rust
pub struct CommandExecutor {
    pub(crate) data: AHashMap<String, Value>,
    pub(crate) expirations: AHashMap<String, VirtualTime>,
    pub(crate) current_time: VirtualTime,
    pub(crate) access_times: AHashMap<String, VirtualTime>,
    pub(crate) key_count: usize,
    pub(crate) commands_processed: usize,
    pub(crate) simulation_start_epoch: i64,      // seconds
    pub(crate) simulation_start_epoch_ms: i64,    // milliseconds
    pub(crate) in_transaction: bool,
    pub(crate) queued_commands: Vec<Command>,
    pub(crate) watched_keys: AHashMap<String, Option<Value>>,
    pub(crate) script_cache: super::lua::ScriptCache,
    pub(crate) shared_script_cache: Option<super::lua::SharedScriptCache>,
    pub(crate) config: config_ops::ServerConfig,
}
```

Each shard owns one `CommandExecutor`. The executor processes commands sequentially —
no concurrency within a shard. Concurrency comes from having multiple shards.

### Execution dispatch

**File:** `src/redis/executor/mod.rs` — dispatch match arms

Operations are split across files:
- `string_ops.rs` — GET, SET, APPEND, INCR, etc.
- `key_ops.rs` — DEL, EXISTS, EXPIRE, TTL, etc.
- `list_ops.rs` — LPUSH, RPUSH, LPOP, RPOP, LRANGE, etc.
- `set_ops.rs` — SADD, SREM, SMEMBERS, SCARD, SPOP, etc.
- `hash_ops.rs` — HSET, HGET, HDEL, HGETALL, etc.
- `sorted_set_ops.rs` — ZADD, ZRANGE, ZSCORE, ZRANK, etc.
- `scan_ops.rs` — SCAN, HSCAN, ZSCAN
- `transaction_ops.rs` — MULTI, EXEC, DISCARD, WATCH
- `script_ops.rs` — EVAL, EVALSHA, SCRIPT
- `acl_ops.rs` — ACL commands
- `config_ops.rs` — CONFIG GET/SET

---

## 6. Actor Communication Patterns

### Request-Response (most commands)

```rust
let (tx, rx) = oneshot::channel();
shard_tx.send(ShardMessage::Command { cmd, response_tx: tx }).await;
let response = rx.await;
```

### Fire-and-Forget (batch operations)

```rust
shard_tx.send(ShardMessage::BatchCommand { cmd, virtual_time }).await;
// No response channel — best effort
```

### Actor Shutdown

```rust
enum Message {
    DoWork(Work),
    Shutdown { response: oneshot::Sender<()> },
}

async fn run(mut self) {
    while let Some(msg) = self.rx.recv().await {
        match msg {
            Message::DoWork(w) => self.handle_work(w).await,
            Message::Shutdown { response } => {
                self.cleanup().await;
                let _ = response.send(());
                break;
            }
        }
    }
}
```

### Bridging Sync to Async

When connecting sync code (command execution) to async actors:

```rust
// Use std::sync::mpsc for fire-and-forget from sync context
let (tx, rx) = std::sync::mpsc::channel();

// Bridge task drains sync channel into async actor
async fn bridge(rx: Receiver<Delta>, actor: ActorHandle) {
    loop {
        if let Some(delta) = rx.recv_timeout(Duration::from_millis(50)) {
            actor.send(delta);
        }
        if shutdown.load(Ordering::SeqCst) { break; }
    }
}
```

---

## 7. TIME Command — Special Case

`TIME` returns real wall-clock time via `SystemTime::now()` at the `sharded_actor.rs`
level, NOT virtual time from the executor. This is intentional — Redis TIME returns
real server time.

---

## 8. perf_config.toml — Shard Count

**Critical:** Two separate config files exist:

| File | `num_shards` | Used by |
|------|-------------|---------|
| Root `perf_config.toml` | 1 | Tcl tests, Lua scripts (need single shard for MULTI/EXEC) |
| `docker-benchmark/perf_config.toml` | 16 | Docker benchmarks (throughput) |

Changing one does NOT affect the other. The Docker build copies from `docker-benchmark/`.

---

## Anti-patterns

- **`Arc<Mutex<State>>`** — Actors own state. If you need shared state, you need another actor or a message.
- **Blocking in async context** — Never call blocking I/O in a tokio task. Use `spawn_blocking` or message an actor.
- **Shared mutable state between connection and shard** — Connection handler owns MULTI/EXEC state. Executor owns key-value state. They communicate via messages only.
- **Sending to self** — An actor should never send messages to its own channel. Process state changes directly.
- **Unbounded channels** — Always use bounded mpsc channels with backpressure. Unbounded channels can OOM under load.
- **Holding response_tx across await points** — Send the response before yielding. Holding a oneshot sender across `.await` can cause timeouts.
