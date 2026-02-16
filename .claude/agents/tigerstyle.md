---
name: tigerstyle
description: TigerStyle coding discipline — assertions, checked arithmetic, design-by-contract
user_invocable: true
---

# TigerStyle — redis-rust

You are about to modify executor or data structure code. This skill injects the TigerStyle
coding discipline. Every example below is drawn from or modeled on the actual codebase.

---

## 1. What is TigerStyle?

TigerStyle is a coding discipline from TigerBeetle (https://tigerstyle.dev), itself
drawing from NASA/JPL's "Power of 10" rules (Holzmann, 2006) and Bertrand Meyer's
Design by Contract (1986). The general principles predate TigerBeetle — they are
established software engineering practices. TigerBeetle codified them for
systems programming in a way we adopt directly.

The core principle: **make bugs impossible at the type/assertion level, not just unlikely.**

Three pillars:
1. **Assertions** — preconditions at entry, postconditions after mutation, invariants always
2. **Checked arithmetic** — no silent overflow, ever
3. **Explicit resource bounds** — no unbounded allocation, no silent failures

These are not project-specific conventions — they are universal safety practices.
The examples below show how we apply them in this codebase.

---

## 2. Assertions

### Preconditions (at function entry)

Assert what must be true before the function can execute correctly:

```rust
// GOOD: Assert preconditions
fn lindex(&self, key: &str, index: i64) -> Result<Option<&Value>> {
    debug_assert!(!key.is_empty(), "Precondition: key must not be empty");

    // ... implementation
}

// BAD: No preconditions, silent wrong behavior
fn lindex(&self, key: &str, index: i64) -> Option<&Value> {
    self.data.get(key).and_then(|v| v.as_list()?.get(index as usize))
}
```

### Postconditions (after mutation)

Assert what must be true after the function has done its work:

```rust
// GOOD: Assert postconditions
fn hset(&mut self, key: &str, field: String, value: String) -> Result<i64> {
    debug_assert!(!key.is_empty(), "Precondition: key must not be empty");

    let hash = self.data.entry(key.to_string())
        .or_insert_with(|| Value::Hash(AHashMap::new()));

    let is_new = if let Value::Hash(ref mut map) = hash {
        let existed = map.insert(field.clone(), value.clone());
        existed.is_none()
    } else {
        return Err(Error::WrongType);
    };

    // Postcondition: field must now exist with correct value
    debug_assert!(
        matches!(self.data.get(key), Some(Value::Hash(m)) if m.contains_key(&field)),
        "Postcondition: field must exist after HSET"
    );

    Ok(if is_new { 1 } else { 0 })
}
```

### Invariants (always true)

Structural properties that must hold at all times:

```rust
// GOOD: Invariant verification method
impl CommandExecutor {
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        debug_assert_eq!(
            self.data.len(),
            self.key_count,
            "Invariant: key_count must match data.len()"
        );

        for (key, expiry) in &self.expirations {
            debug_assert!(
                self.data.contains_key(key),
                "Invariant: expiration key '{key}' must exist in data"
            );
        }
    }
}
```

### Call verify_invariants() after every mutation

```rust
pub fn del(&mut self, keys: &[String]) -> i64 {
    let count = /* ... deletion logic ... */;

    #[cfg(debug_assertions)]
    self.verify_invariants();

    count
}
```

---

## 3. Checked Arithmetic

**NEVER use raw `+`, `-`, `*`, `/` on integers.** Silent overflow is a bug factory.

### The Pattern

```rust
// GOOD: Checked arithmetic with explicit error
fn incr_by_impl(&mut self, key: &str, increment: i64) -> Result<i64> {
    let current = self.get_i64(key)?;

    let new_value = current.checked_add(increment)
        .ok_or_else(|| Error::Overflow)?;

    self.set_i64(key, new_value);

    // Postcondition
    debug_assert_eq!(
        self.get_i64(key).unwrap(),
        new_value,
        "Postcondition: stored value must equal computed value"
    );

    Ok(new_value)
}

// BAD: Silent overflow
fn incr_by_impl(&mut self, key: &str, increment: i64) -> i64 {
    let current = self.get_i64(key);
    let new_value = current + increment;  // WRAPS ON OVERFLOW!
    self.set_i64(key, new_value);
    new_value
}
```

### When to use which variant

| Method | Use when |
|--------|----------|
| `checked_add`, `checked_sub`, `checked_mul`, `checked_div` | Overflow is an error — return `Result` |
| `saturating_add`, `saturating_sub` | Clamping to min/max is the correct behavior |
| `wrapping_add`, `wrapping_sub` | **Never** in this codebase (defeats the purpose) |
| Raw `+`, `-`, `*`, `/` | **Never** on user-controlled or computed integers |

### Common traps

```rust
// TRAP: Casting i64 to usize can lose negative values
let index: i64 = -1;
let uindex = index as usize;  // BAD: wraps to usize::MAX

// GOOD: Handle negative indices explicitly
let uindex = if index >= 0 {
    index as usize
} else {
    let from_end = (-index) as usize;
    len.checked_sub(from_end).ok_or(Error::OutOfRange)?
};
```

---

## 4. Error Handling

### Library code: NEVER unwrap

```rust
// GOOD: Propagate errors
fn process_command(cmd: &[u8]) -> Result<Response, Error> {
    let parsed = parse_command(cmd)?;
    let result = execute(&parsed)?;
    Ok(result)
}

// BAD: Panics in production
fn process_command(cmd: &[u8]) -> Response {
    let parsed = parse_command(cmd).unwrap();  // PANIC!
    execute(&parsed).unwrap()
}
```

### `.expect()` with explanation

Only when failure is provably impossible:

```rust
// ACCEPTABLE: We just inserted this key
self.data.insert(key.clone(), value);
let v = self.data.get(&key).expect("just inserted");
```

### Test code

`.unwrap()` is acceptable. Prefer `.expect("message")` for better diagnostics.

---

## 5. Explicit Resource Bounds

### Bounded collections

```rust
// GOOD: Explicit capacity
let mut buffer = Vec::with_capacity(expected_count);

// GOOD: Bounded queue
let (tx, rx) = mpsc::channel(1024);  // bounded

// BAD: Unbounded growth
let mut buffer = Vec::new();  // could grow to OOM
let (tx, rx) = mpsc::unbounded_channel();  // unbounded
```

### No hidden allocation in hot paths

```rust
// GOOD: Pre-allocated, reused buffer
fn encode_response(&self, buf: &mut BytesMut) {
    buf.put_slice(b"+OK\r\n");
}

// BAD: Allocates on every call
fn encode_response(&self) -> String {
    format!("+OK\r\n")  // heap allocation
}
```

---

## 6. Control Flow

### Early returns (max 3 levels of nesting)

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

---

## 7. Design-by-Contract Checklist for New Structs

When creating a new stateful struct:

- [ ] Define all invariants as doc comments
- [ ] Implement `verify_invariants()` method
- [ ] Call `verify_invariants()` after every public mutation method
- [ ] Wrap calls in `#[cfg(debug_assertions)]` to avoid release overhead
- [ ] Add precondition `debug_assert!` at entry of every public method
- [ ] Add postcondition `debug_assert!` after every state mutation
- [ ] Use checked arithmetic for all integer operations
- [ ] Return `Result<T, Error>` from all fallible operations

---

## 8. Real Codebase Examples

### Good: `incr_by_impl` pattern (string_ops.rs)

```rust
let new_value = current_value.checked_add(increment)
    .ok_or_else(|| Error::Overflow)?;
// Postcondition verified
```

### Good: Sorted set invariant verification

```rust
#[cfg(debug_assertions)]
fn verify_invariants(&self) {
    debug_assert_eq!(
        self.members.len(), self.scores.len(),
        "members and scores must have same length"
    );
}
```

### Bad (fixed): MSET postcondition checked duplicate pairs

```rust
// OLD (buggy): Checked ALL pairs including duplicates
// MSET key1 val1 key1 val2 -> postcondition failed because it checked key1=val1
debug_assert!(pairs.iter().all(|(k, v)| executor.get(k) == Some(v)));

// FIXED: Check last value per key (last-write-wins for duplicates)
// Build expected: last value per key
let mut expected = HashMap::new();
for (k, v) in &pairs { expected.insert(k, v); }
debug_assert!(expected.iter().all(|(k, v)| executor.get(k) == Some(v)));
```

---

## Anti-patterns

- **Assertions in release builds without `debug_assert!`.** Use `debug_assert!` (zero cost in release) or `#[cfg(debug_assertions)]` blocks. Only use `assert!` for things that should panic in production (e.g., invariant violations that indicate memory corruption).
- **Postconditions that re-derive the result.** The postcondition should check a different property than the computation. If you just re-compute the same thing, you'll get the same bugs.
- **Missing postconditions on collection mutations.** After insert: assert contains. After delete: assert !contains. After clear: assert is_empty.
- **Raw arithmetic on user input.** Redis commands like INCR, DECRBY, EXPIRE accept user-provided integers. Always use `checked_*`.
- **`.unwrap()` in library code.** Use `.expect("reason")` at absolute minimum, prefer `?` propagation.
