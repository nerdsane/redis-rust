# ADR-003: TigerStyle Coding Standards

## Status

Accepted

## Context

Systems programming requires exceptional attention to correctness. Silent failures, undefined behavior, and subtle bugs can cause data loss or security vulnerabilities. Traditional coding practices often:

1. Use `.unwrap()` liberally, masking error paths
2. Ignore integer overflow (wrapping silently)
3. Lack invariant checking (bugs discovered in production)
4. Have implicit assumptions (undocumented preconditions)

TigerBeetle's "Tiger Style" provides a rigorous engineering discipline that prioritizes **Safety > Performance > Developer Experience**. Given our DST-first approach (ADR-001), TigerStyle complements simulation testing with runtime verification.

## Decision

We will adopt **TigerStyle Coding Standards** throughout the codebase:

### 1. Assertions (REQUIRED for every mutation)

```rust
// GOOD: Assert preconditions and postconditions
fn hincrby(&mut self, field: &str, increment: i64) -> Result<i64> {
    debug_assert!(!field.is_empty(), "Precondition: field must not be empty");

    let new_value = self.value.checked_add(increment)
        .ok_or_else(|| Error::Overflow)?;

    self.value = new_value;

    debug_assert_eq!(self.get(field), Some(new_value),
        "Postcondition: value must equal computed result");

    Ok(new_value)
}

// BAD: No assertions, silent failures
fn hincrby(&mut self, field: &str, increment: i64) -> i64 {
    self.value += increment;  // Can overflow!
    self.value
}
```

### 2. Checked Arithmetic (REQUIRED)

```rust
// GOOD: Explicit overflow handling
let new_value = self.value.checked_add(increment)
    .ok_or_else(|| Error::Overflow)?;

// BAD: Silent overflow
let new_value = self.value + increment;

// OK when saturation is correct behavior
let count = count.saturating_add(1);
```

### 3. Design-by-Contract

```rust
impl RedisSortedSet {
    /// Verify all invariants hold - call in debug builds after mutations
    fn verify_invariants(&self) {
        debug_assert_eq!(
            self.members.len(),
            self.sorted_members.len(),
            "Invariant: members and sorted_members must have same length"
        );
        debug_assert!(
            self.is_sorted(),
            "Invariant: sorted_members must be sorted by (score, member)"
        );
    }

    pub fn add(&mut self, member: String, score: f64) {
        // ... mutation logic ...

        #[cfg(debug_assertions)]
        self.verify_invariants();
    }
}
```

### 4. Control Flow

- **Early returns** for error cases
- **No deep nesting** (max 3 levels)
- **Explicit allocations** with `Vec::with_capacity`
- **No panics** in production paths

### 5. Error Handling

```rust
// GOOD: Explicit Result
fn parse_integer(s: &str) -> Result<i64, ParseError> {
    s.parse().map_err(|_| ParseError::InvalidInteger)
}

// BAD: Panic on error
fn parse_integer(s: &str) -> i64 {
    s.parse().unwrap()
}
```

## Consequences

### Positive

- **Bug detection**: Assertions catch invariant violations in tests
- **Documentation**: Assertions document assumptions
- **Debugging**: Clear error messages with assertion context
- **Refactoring safety**: Invariants verified after changes
- **Production safety**: No silent failures or undefined behavior

### Negative

- **Verbosity**: More code for assertions and error handling
- **Debug overhead**: Assertions add runtime cost in debug builds
- **Learning curve**: Team must adopt new patterns
- **Initial slowdown**: Writing assertions takes time

### Risks

- **False confidence**: Assertions may not cover all invariants
- **Disabled in release**: `debug_assert!` doesn't run in production
- **Over-assertion**: Too many assertions can obscure logic

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-03 | Initial ADR created | TigerStyle adopted for safety-first engineering |
| 2026-01-03 | Use debug_assert! not assert! | Avoid production overhead while catching bugs in tests |
| 2026-01-04 | Add verify_invariants() pattern | Centralize invariant checking for complex structs |
| 2026-01-05 | Require checked arithmetic everywhere | Integer overflow is undefined behavior in some contexts |
| 2026-01-06 | Document allowed clippy lints | Some stylistic lints conflict with TigerStyle |

## Implementation Status

### Implemented

| Component | Location | Status |
|-----------|----------|--------|
| Checked arithmetic | `src/redis/commands.rs` | INCR/DECR use checked_add/sub |
| Invariant verification | `src/redis/data/{hash,sorted_set,list}.rs` | SortedSet, Hash, List verify invariants |
| Error handling | Throughout | Result<T, E> instead of panics |
| Clippy configuration | `src/lib.rs` | Allowed lints documented |
| Precondition assertions | Command handlers | Input validation assertions |
| Postcondition assertions | Data mutations | State verification after changes |

### Validated

- All INCR/DECR operations use checked arithmetic
- No `.unwrap()` in production code paths
- Invariant checks run in debug builds
- DST tests with assertions enabled catch bugs

### Formally Verified

| Component | Location | Status |
|-----------|----------|--------|
| TLA+ Specifications | `specs/tla/*.tla` | 4 specs: GossipProtocol, ReplicationConvergence, AntiEntropy, StreamingPersistence |
| Stateright Models | `src/stateright/*.rs` | 3 models: CrdtMergeModel, WriteBufferModel, AntiEntropyModel |
| Kani Proofs | `src/replication/lattice.rs` | CRDT merge commutativity, associativity, idempotence proven |

### Not Yet Implemented

| Component | Notes |
|-----------|-------|
| Fuzzing integration | No libfuzzer/AFL coverage |
| Mutation testing | No cargo-mutants integration |

## References

- [TigerBeetle TIGER_STYLE.md](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md)
- [TigerBeetle Safety](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/concepts/safety.md)
- [Design by Contract](https://en.wikipedia.org/wiki/Design_by_contract)
- [Rust Error Handling](https://doc.rust-lang.org/book/ch09-00-error-handling.html)
