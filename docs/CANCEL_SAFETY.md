# Cancel Safety and Async Patterns

This guide covers async safety patterns for redis-rust, ensuring correct behavior under cancellation and concurrent access.

## Overview

Async Rust has subtle pitfalls that can cause:
- **Data loss** on cancellation
- **Deadlocks** from improper lock usage
- **Resource leaks** from orphaned tasks
- **Undefined behavior** from improper future recreation

## Forbidden Patterns

### 1. `tokio::sync::Mutex` in Hot Paths

**Problem**: `tokio::sync::Mutex` has overhead and can cause deadlocks when held across await points in complex scenarios.

```rust
// FORBIDDEN in hot paths
use tokio::sync::Mutex;

struct HotPath {
    data: Mutex<Data>,  // Bad for performance-critical code
}

impl HotPath {
    async fn process(&self) {
        let guard = self.data.lock().await;  // Holding across await is dangerous
        some_async_operation().await;  // If cancelled here, lock behavior is subtle
    }
}
```

**Solution**: Use message passing or `parking_lot::Mutex` for short critical sections:

```rust
// GOOD: Actor pattern with message passing
struct HotPathActor {
    data: Data,  // Owned, not shared
    rx: mpsc::Receiver<Message>,
}

// GOOD: parking_lot for synchronous critical sections
use parking_lot::Mutex;

struct HotPath {
    data: Mutex<Data>,
}

impl HotPath {
    fn process(&self) -> Data {
        let guard = self.data.lock();  // Synchronous, brief
        guard.clone()  // Release lock before any async work
    }
}
```

### 2. `JoinHandle::abort()`

**Problem**: `abort()` can leave state inconsistent if the task was in the middle of a multi-step operation.

```rust
// FORBIDDEN
let handle = tokio::spawn(async {
    step_one().await;  // Completed
    step_two().await;  // abort() here leaves partial state
    step_three().await;
});

handle.abort();  // Dangerous!
```

**Solution**: Use cancellation tokens for cooperative shutdown:

```rust
// GOOD: Cooperative cancellation
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();
let task_token = token.clone();

let handle = tokio::spawn(async move {
    loop {
        tokio::select! {
            _ = task_token.cancelled() => {
                cleanup().await;
                break;
            }
            result = do_work() => {
                // Process result
            }
        }
    }
});

// Graceful shutdown
token.cancel();
handle.await.unwrap();
```

### 3. Recreating Futures in `select!`

**Problem**: If a future is recreated each iteration, partial progress is lost.

```rust
// FORBIDDEN: Future recreated each iteration
loop {
    tokio::select! {
        result = expensive_operation() => {  // New future each time!
            // If other branch wins, this work is discarded
        }
        _ = shutdown.recv() => {
            break;
        }
    }
}
```

**Solution**: Create futures outside the loop or use `tokio::pin!`:

```rust
// GOOD: Future created once, polled to completion
let operation = expensive_operation();
tokio::pin!(operation);

loop {
    tokio::select! {
        result = &mut operation => {
            // Process result
            break;
        }
        _ = shutdown.recv() => {
            // Operation will complete in background or be dropped
            break;
        }
    }
}
```

### 4. Holding Locks Across Await

```rust
// FORBIDDEN: Lock held across await
async fn bad_pattern(data: &Mutex<Data>) {
    let mut guard = data.lock();
    network_call().await;  // Lock held during I/O!
    guard.update();
}
```

**Solution**: Release lock before await, re-acquire after:

```rust
// GOOD: Release before await
async fn good_pattern(data: &Mutex<Data>) {
    let snapshot = {
        let guard = data.lock();
        guard.snapshot()
    };  // Lock released here

    let result = network_call(snapshot).await;

    {
        let mut guard = data.lock();
        guard.apply(result);
    }
}
```

## Cancel-Safe Patterns

### 1. Actor Model

Actors own their state exclusively and communicate via channels:

```rust
struct PersistenceActor {
    state: PersistenceState,  // Owned, not shared
    rx: mpsc::Receiver<Message>,
    shutdown: CancellationToken,
}

impl PersistenceActor {
    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;  // Check shutdown first

                _ = self.shutdown.cancelled() => {
                    self.graceful_shutdown().await;
                    break;
                }

                Some(msg) = self.rx.recv() => {
                    self.handle_message(msg).await;
                }
            }
        }
    }

    async fn graceful_shutdown(&mut self) {
        // Flush pending writes
        self.flush().await;
        // Close resources
        self.close().await;
    }
}
```

### 2. Oneshot Response Pattern

For request-response within actors:

```rust
enum Message {
    Get {
        key: String,
        response: oneshot::Sender<Option<Value>>,
    },
    Set {
        key: String,
        value: Value,
        response: oneshot::Sender<Result<(), Error>>,
    },
    Shutdown {
        response: oneshot::Sender<()>,
    },
}

impl Actor {
    async fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::Get { key, response } => {
                let value = self.state.get(&key).cloned();
                let _ = response.send(value);  // Ignore if receiver dropped
            }
            Message::Shutdown { response } => {
                self.cleanup().await;
                let _ = response.send(());
            }
        }
    }
}
```

### 3. Bridging Sync to Async

When command execution (sync) needs to trigger persistence (async):

```rust
// Use std::sync::mpsc for fire-and-forget from sync context
fn create_bridge() -> (SyncSender, AsyncReceiver) {
    let (tx, rx) = std::sync::mpsc::channel();
    (tx, rx)
}

// Bridge task runs in async context
async fn bridge_task(
    rx: std::sync::mpsc::Receiver<Delta>,
    actor_tx: mpsc::Sender<ActorMessage>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;

            _ = shutdown.cancelled() => {
                break;
            }

            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                // Drain sync channel in batches
                while let Ok(delta) = rx.try_recv() {
                    let _ = actor_tx.send(ActorMessage::Delta(delta)).await;
                }
            }
        }
    }
}
```

### 4. Bounded Channels with Backpressure

Never use unbounded channels in production:

```rust
// GOOD: Bounded channel with explicit backpressure
let (tx, rx) = mpsc::channel(1000);  // Explicit capacity

async fn send_with_backpressure(tx: &mpsc::Sender<Data>, data: Data) -> Result<()> {
    match tx.try_send(data) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(data)) => {
            // Backpressure: wait or drop
            metrics::increment("channel.backpressure");
            tx.send(data).await.map_err(|_| Error::ChannelClosed)
        }
        Err(TrySendError::Closed(_)) => {
            Err(Error::ChannelClosed)
        }
    }
}
```

### 5. Timeout Wrapping

Always wrap external operations with timeouts:

```rust
use tokio::time::timeout;

async fn network_call_safe(req: Request) -> Result<Response> {
    timeout(Duration::from_secs(30), network_call(req))
        .await
        .map_err(|_| Error::Timeout)?
}
```

## Testing Cancel Safety

### Cancellation Test Pattern

```rust
#[tokio::test]
async fn test_cancellation_safety() {
    let actor = spawn_actor();

    // Start an operation
    let operation = actor.expensive_operation();

    // Cancel mid-operation
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(10)) => {
            // Operation cancelled here
        }
        _ = operation => {
            panic!("Should have been cancelled");
        }
    }

    // Verify actor is still functional
    let result = actor.simple_operation().await;
    assert!(result.is_ok());

    // Verify no data corruption
    actor.verify_invariants().await;
}
```

### DST Cancellation Faults

```rust
#[test]
fn test_cancellation_under_faults() {
    for seed in 0..50 {
        let harness = SimulationHarness::new(seed)
            .with_faults(FaultConfig::builder()
                .with_fault("TASK_CANCEL", 0.1)  // Random cancellation
                .with_fault("CHANNEL_CLOSE", 0.05)
                .build());

        harness.run_scenario(|ctx| async move {
            // Operations that might be cancelled
            for _ in 0..100 {
                let _ = ctx.do_operation().await;  // May be cancelled
            }

            // System must remain consistent
            ctx.verify_invariants().await;
            Ok(())
        });
    }
}
```

## Checklist

Before merging async code, verify:

- [ ] No `tokio::sync::Mutex` in hot paths
- [ ] No `JoinHandle::abort()` - use cancellation tokens
- [ ] No future recreation in `select!` loops
- [ ] No locks held across await points
- [ ] All channels are bounded
- [ ] All external operations have timeouts
- [ ] Graceful shutdown paths tested
- [ ] Cancellation safety tested with DST

## Resources

- [Tokio Tutorial: Shared State](https://tokio.rs/tokio/tutorial/shared-state)
- [Async Book: Cancellation](https://rust-lang.github.io/async-book/06_multiple_futures/03_select.html)
- [Alice Ryhl: Actors with Tokio](https://ryhl.io/blog/actors-with-tokio/)
