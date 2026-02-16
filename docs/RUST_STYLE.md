# redis-rust Rust Style Guide

This guide supplements [TigerStyle](https://tigerstyle.dev) with patterns from Firecracker, DataFusion, and FoundationDB. All contributions must follow these standards.

## Quick Reference

| Pattern | Rule |
|---------|------|
| Error handling | Propagate via `?`, never `unwrap()` in library code |
| File size | Maximum 500 lines per file (document deviations) |
| Clone avoidance | Return `&self` for reads, use `Arc<T>` for sharing |
| Iterators | Prefer combinators over explicit loops |
| Assertions | 2+ assertions per function (preconditions/postconditions) |
| Arithmetic | Use `checked_*` methods, never assume no overflow |

## 1. Error Handling

### Library Code

- **MUST** propagate errors with `?` operator
- **NEVER** use `.unwrap()` in production paths
- **USE** `.expect()` only when failure is provably impossible (with explanation)

```rust
// GOOD: Propagate errors
fn process_command(cmd: &[u8]) -> Result<Response, Error> {
    let parsed = parse_command(cmd)?;
    let result = execute(&parsed)?;
    Ok(result)
}

// BAD: Panics in production
fn process_command(cmd: &[u8]) -> Response {
    let parsed = parse_command(cmd).unwrap();  // Will panic!
    execute(&parsed).unwrap()
}
```

### Test Code

- `.unwrap()` is acceptable in tests
- Prefer `.expect("message")` for better error messages

## 2. TigerStyle Assertions

Every function that mutates state must have assertions:

```rust
// GOOD: Preconditions and postconditions
fn hincrby(&mut self, field: &str, increment: i64) -> Result<i64> {
    // Precondition
    debug_assert!(!field.is_empty(), "field must not be empty");

    let current = self.values.get(field).copied().unwrap_or(0);
    let new_value = current.checked_add(increment)
        .ok_or(Error::Overflow)?;

    self.values.insert(field.to_string(), new_value);

    // Postcondition
    debug_assert_eq!(
        self.values.get(field),
        Some(&new_value),
        "value must be stored"
    );

    Ok(new_value)
}
```

### Invariant Verification

Stateful structs should have a `verify_invariants()` method:

```rust
impl RedisSortedSet {
    /// Verify all invariants - call in debug builds after mutations
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        debug_assert_eq!(
            self.members.len(),
            self.scores.len(),
            "members and scores must have same length"
        );
        // Additional invariants...
    }

    pub fn zadd(&mut self, member: String, score: f64) {
        // ... mutation logic ...

        #[cfg(debug_assertions)]
        self.verify_invariants();
    }
}
```

## 3. Checked Arithmetic

**NEVER** assume arithmetic won't overflow:

```rust
// GOOD: Explicit overflow handling
let new_value = current.checked_add(increment)
    .ok_or(Error::Overflow)?;

// GOOD: Saturating when that's the correct behavior
let capped = value.saturating_add(delta);

// BAD: Silent overflow in debug, wrap in release
let new_value = current + increment;
```

## 4. File Size Limits

- **Target**: 500 lines maximum per file
- **Exceptions**: Must be documented in `docs/adr/gaps/`

Current known deviations:
- `src/redis/commands.rs` - Command implementations (will be split)
- `src/redis/data.rs` - Data structures (will be split)
- `src/redis/tests.rs` - Test suite

## 5. DST-Compatible Design

Every I/O component must be simulatable:

```rust
// GOOD: I/O through trait abstraction
pub trait ObjectStore: Send + Sync {
    fn put(&self, key: &str, data: &[u8]) -> impl Future<Output = Result<()>>;
    fn get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>>>;
}

// Production implementation
pub struct S3ObjectStore { ... }

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

## 6. Clone Avoidance

Minimize unnecessary clones:

```rust
// GOOD: Return reference
fn get_value(&self, key: &str) -> Option<&Value> {
    self.map.get(key)
}

// GOOD: Use Arc for shared ownership
fn share_state(&self) -> Arc<State> {
    Arc::clone(&self.state)
}

// BAD: Unnecessary clone
fn get_value(&self, key: &str) -> Option<Value> {
    self.map.get(key).cloned()  // Avoid when reference suffices
}
```

## 7. Iterator Patterns

Prefer functional combinators over explicit loops:

```rust
// GOOD: Functional style
let sum: i64 = values.iter()
    .filter(|v| v.is_valid())
    .map(|v| v.amount())
    .sum();

// ACCEPTABLE: When logic is complex
let mut sum = 0i64;
for value in &values {
    if value.is_valid() {
        // Complex multi-step logic here
        sum = sum.checked_add(value.amount())
            .ok_or(Error::Overflow)?;
    }
}
```

## 8. Naming Conventions

| Pattern | Example |
|---------|---------|
| Functions returning `Result` | `fn parse() -> Result<T>` |
| Functions that can panic | `fn must_parse() -> T` (avoid in library code) |
| Async functions | `async fn fetch()` |
| Builder methods | `fn with_timeout(self, t: Duration) -> Self` |
| Conversion | `fn into_bytes(self) -> Bytes` |

## 9. Import Organization

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

## 10. Control Flow

- Prefer early returns over deep nesting
- Maximum 3 levels of indentation (excluding match arms)

```rust
// GOOD: Early returns
fn process(input: &Input) -> Result<Output> {
    if input.is_empty() {
        return Ok(Output::empty());
    }

    let validated = validate(input)?;
    let result = compute(validated)?;
    Ok(result)
}

// BAD: Deep nesting
fn process(input: &Input) -> Result<Output> {
    if !input.is_empty() {
        if let Ok(validated) = validate(input) {
            if let Ok(result) = compute(validated) {
                return Ok(result);
            }
        }
    }
    Ok(Output::empty())
}
```

## 11. Documentation

- Public API must have doc comments
- Include examples for non-trivial functions
- Document panics, errors, and safety requirements

```rust
/// Increments a hash field by the given amount.
///
/// # Errors
///
/// Returns `Error::Overflow` if the result would overflow i64.
///
/// # Example
///
/// ```
/// let mut hash = RedisHash::new();
/// hash.hincrby("counter", 1)?;
/// ```
pub fn hincrby(&mut self, field: &str, increment: i64) -> Result<i64> {
    // ...
}
```

## Checklist for Code Review

- [ ] No `.unwrap()` in library code
- [ ] Assertions for preconditions and postconditions
- [ ] Checked arithmetic for all integer operations
- [ ] DST-compatible I/O abstractions
- [ ] No files exceeding 500 lines (or documented exception)
- [ ] Clippy passes with `-D warnings`
- [ ] Tests cover error paths
