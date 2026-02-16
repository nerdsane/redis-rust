---
name: rust-dev
description: Rust development patterns, style guide, and project-specific conventions
user_invocable: true
---

# Rust Development — redis-rust

You are about to write Rust code in this project. This skill is almost entirely
project-specific conventions (not general Rust guidance). For general Rust idioms, use
your training knowledge. For our project's specific patterns, conventions, and
gotchas, use the sections below (drawn from `docs/RUST_STYLE.md` and the codebase).

---

## 1. Quick Reference

| Pattern | Rule |
|---------|------|
| Error handling | Propagate via `?`, never `.unwrap()` in library code |
| File size | Maximum 500 lines per file (document deviations) |
| Clone avoidance | Return `&self` for reads, use `Arc<T>` for sharing |
| Iterators | Prefer combinators over explicit loops |
| Assertions | 2+ assertions per function (preconditions/postconditions) |
| Arithmetic | Use `checked_*` methods, never assume no overflow |

---

## 2. Error Handling

### Library code

```rust
// GOOD: Propagate with ?
fn process_command(cmd: &[u8]) -> Result<Response, Error> {
    let parsed = parse_command(cmd)?;
    let result = execute(&parsed)?;
    Ok(result)
}

// BAD: Panics in production
fn process_command(cmd: &[u8]) -> Response {
    parse_command(cmd).unwrap()
}
```

### Test code

`.unwrap()` acceptable. Prefer `.expect("message")` for better error messages.

---

## 3. File Size Limits

**Target:** 500 lines maximum per file. Document exceptions in `docs/adr/gaps/`.

Known deviations:
- `src/redis/commands.rs` — Command implementations
- `src/redis/command.rs` — 100+ variants in the Command enum

If a file exceeds 500 lines, split it. The executor already demonstrates this:
`mod.rs` + 11 `*_ops.rs` files.

---

## 4. Clone Avoidance

```rust
// GOOD: Return reference
fn get_value(&self, key: &str) -> Option<&Value> {
    self.map.get(key)
}

// GOOD: Arc for shared ownership
fn share_state(&self) -> Arc<State> {
    Arc::clone(&self.state)
}

// BAD: Unnecessary clone
fn get_value(&self, key: &str) -> Option<Value> {
    self.map.get(key).cloned()  // Avoid when reference suffices
}
```

---

## 5. Iterator Patterns

```rust
// GOOD: Functional combinators
let sum: i64 = values.iter()
    .filter(|v| v.is_valid())
    .map(|v| v.amount())
    .sum();

// ACCEPTABLE: When logic is complex (e.g., checked arithmetic)
let mut sum = 0i64;
for value in &values {
    if value.is_valid() {
        sum = sum.checked_add(value.amount())
            .ok_or(Error::Overflow)?;
    }
}
```

---

## 6. Import Organization

Enforced by `rustfmt.toml`:

```rust
// 1. Standard library
use std::collections::HashMap;
use std::sync::Arc;

// 2. External crates
use bytes::Bytes;
use tokio::sync::mpsc;

// 3. Crate modules
use crate::error::Error;
use crate::types::Value;
```

---

## 7. Control Flow

- Early returns over deep nesting
- Maximum 3 levels of indentation (excluding match arms)

```rust
// GOOD
fn process(input: &Input) -> Result<Output> {
    if input.is_empty() {
        return Ok(Output::empty());
    }
    let validated = validate(input)?;
    compute(validated)
}
```

---

## 8. DST-Compatible Design

Every I/O component must be simulatable:

```rust
// GOOD: Trait abstraction for I/O
pub trait ObjectStore: Send + Sync {
    fn put(&self, key: &str, data: &[u8]) -> impl Future<Output = Result<()>>;
    fn get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>>>;
}

// Production implementation
pub struct S3ObjectStore { /* ... */ }

// Test implementation with fault injection
pub struct SimulatedObjectStore {
    inner: InMemoryStore,
    fault_config: FaultConfig,
}

// BAD: Direct I/O that can't be simulated
async fn save_data(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data)?;  // Can't inject faults!
    Ok(())
}
```

---

## 9. Borrow Checker Patterns

### Extract-then-assert (DST harnesses)

Cannot call `self.assert_*()` (`&mut self`) while holding `self.shadow.get()` (`&self`).

```rust
// BAD: borrow conflict
let expected = self.shadow.get(&key);  // borrows &self
self.assert_result(result, expected);   // borrows &mut self — ERROR

// GOOD: extract into local, borrow ends
let expected = self.shadow.get(&key).cloned();
self.assert_result(result, expected);  // fine
```

### Temporary variables for complex borrows

```rust
// BAD: Can't borrow self mutably twice
let a = self.get_value(&key1);
self.set_value(&key2, a);  // ERROR: self still borrowed

// GOOD: Clone or extract first
let a = self.get_value(&key1).clone();
self.set_value(&key2, a);
```

---

## 10. Feature Flags

**File:** `Cargo.toml`

```toml
[features]
default = ["lua"]
simulation = []               # DST simulation support
compression = []              # Zstd compression
lua = ["dep:mlua"]            # Lua scripting
s3 = []                       # S3 object store
tls = ["dep:tokio-rustls", "dep:rustls-pemfile", "dep:x509-parser"]
acl = ["dep:sha2"]
security = ["tls", "acl"]
datadog = ["dep:dogstatsd", "dep:opentelemetry", ...]

# Performance optimization flags (individually measurable)
opt-single-key-alloc = []     # P0: Single allocation in set_direct
opt-static-responses = []     # P1: Static OK/PONG responses
opt-zero-copy-get = []        # P2: Zero-copy GET response
opt-itoa-encode = ["itoa"]    # P3: Fast integer encoding
opt-fxhash-routing = []       # P4: FxHash for shard routing
opt-atoi-parse = ["atoi"]     # P5: Fast integer parsing
opt-all = [...]               # All performance flags
```

Use `#[cfg(feature = "simulation")]` to gate simulation-only code.

---

## 11. Async Patterns

### Actor message loop

```rust
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

### Never block in async context

```rust
// BAD: Blocks the tokio runtime thread
let data = std::fs::read("file.txt")?;

// GOOD: Use async I/O or spawn_blocking
let data = tokio::fs::read("file.txt").await?;
// or
let data = tokio::task::spawn_blocking(|| std::fs::read("file.txt")).await??;
```

---

## 12. Project-Specific Conventions

### Two parsers must stay in sync

When adding a new command:
1. Add parsing in `src/redis/parser.rs` (the RESP protocol parser)
2. Add parsing in `src/redis/commands.rs` (the zero-copy parser)

Both must handle the same commands. If they diverge, production vs simulation behavior
will differ.

### Command::Set has many fields

The `Command::Set` variant has optional fields: `ex`, `px`, `exat`, `pxat`, `nx`, `xx`,
`get`, `keepttl`. There are ~25+ struct literal constructions across the codebase.

When adding a new field to `Command::Set`:
1. Search for `Command::Set {` across all files
2. Update EVERY occurrence
3. Common locations: test files, `script_ops.rs`, `parser.rs`, `commands.rs`, DST harnesses

### Two `perf_config.toml` files

| File | `num_shards` | Purpose |
|------|-------------|---------|
| Root `perf_config.toml` | 1 | Tcl tests, Lua scripts |
| `docker-benchmark/perf_config.toml` | 16 | Docker benchmarks |

Changing one does NOT affect the other.

### Error message format (for Tcl compatibility)

Redis error formats that the Tcl test suite checks with glob matching:
- `ERR wrong number of arguments for 'xxx' command`
- `ERR value is not an integer or out of range`
- `ERR value is not a valid float`
- `ERR syntax error`
- `WRONGTYPE Operation against a key holding the wrong kind of value`

`connection_optimized.rs::encode_error_into` prepends `ERR `. Don't double it.

### Epoch timestamps in executor

```rust
pub(crate) simulation_start_epoch: i64,      // seconds precision
pub(crate) simulation_start_epoch_ms: i64,    // millisecond precision
```

Use `simulation_start_epoch_ms` for ms-precision commands (PEXPIRE, PTTL).
Use `simulation_start_epoch` for second-precision commands (EXPIRE, TTL).

---

## 13. Adding a New Command (Checklist)

1. **`src/redis/command.rs`** — Add variant to `Command` enum. Update:
   - `get_primary_key()` — returns the key for shard routing
   - `get_keys()` — returns all keys the command touches
   - `name()` — returns the command name string
   - `is_read_only()` — true if the command doesn't mutate state

2. **`src/redis/parser.rs`** — Add RESP parsing

3. **`src/redis/commands.rs`** — Add zero-copy parsing (must stay in sync with parser.rs)

4. **`src/redis/executor/*_ops.rs`** — Implement execution logic in the appropriate ops file

5. **`src/redis/executor/mod.rs`** — Add dispatch arm in the main execute match

6. **If all-shard command** — Add aggregation in `src/production/sharded_actor.rs`

7. **If adding fields to Set variant** — Update ALL ~25+ struct literals across test files

---

## 14. Cargo Conventions

```bash
# Run all tests
cargo test --release

# Run specific DST tests
cargo test --lib executor_dst -- --nocapture

# Run with specific feature
cargo test --features simulation

# Run ignored tests (Stateright model checking)
cargo test stateright -- --ignored --nocapture

# Run Tcl compatibility tests
./scripts/run-redis-compat.sh

# Clippy (must pass with -D warnings)
cargo clippy -- -D warnings
```

### Release profile

```toml
[profile.release]
lto = "thin"
codegen-units = 1
panic = "abort"
strip = true
```

---

## Anti-patterns

- **`.unwrap()` in library code.** Use `?` or `.expect("reason")`.
- **Files exceeding 500 lines.** Split into modules.
- **`Arc<Mutex<>>` for actor state.** Actors own their state. Use messages.
- **`std::time` in executor code.** Use `VirtualTime` for simulation compatibility.
- **Uniform key distribution in tests.** Use Zipfian (`skew: 1.0`).
- **Forgetting to update both parsers.** `parser.rs` and `commands.rs` must stay in sync.
- **Raw integer arithmetic.** Use `checked_add`, `checked_sub`, etc.
