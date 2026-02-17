# ADR-007: Feature Flag Optimization Strategy

## Status

Accepted

## Context

Performance optimization is risky:

1. **Premature optimization**: Changes that don't actually improve performance
2. **Regression risk**: "Optimizations" that break correctness
3. **Maintenance burden**: Complex code that's hard to understand
4. **Measurement difficulty**: Hard to attribute improvements to specific changes

We need a disciplined approach to optimization that:
- Allows incremental, measurable changes
- Preserves the ability to disable optimizations
- Enables A/B testing of different approaches
- Maintains code clarity

## Decision

We will use **Cargo feature flags** for all performance optimizations:

### Feature Flag Structure

```toml
[features]
default = ["lua"]

# Code-level optimization flags (default OFF for safety)
# Enable incrementally to measure impact
opt-single-key-alloc = []     # P0: Single allocation in set_direct
opt-static-responses = []     # P1: Static OK/PONG responses
opt-zero-copy-get = []        # P2: Zero-copy GET response
opt-itoa-encode = ["itoa"]    # P3: Fast integer encoding
opt-fxhash-routing = []       # P4: FxHash for shard routing
opt-atoi-parse = ["atoi"]     # P5: Fast integer parsing

# Enable all optimizations
opt-all = [
    "opt-single-key-alloc",
    "opt-static-responses",
    "opt-zero-copy-get",
    "opt-itoa-encode",
    "opt-fxhash-routing",
    "opt-atoi-parse",
]
```

### Usage Pattern

```rust
#[cfg(feature = "opt-static-responses")]
pub static OK_RESPONSE: &[u8] = b"+OK\r\n";

#[cfg(not(feature = "opt-static-responses"))]
pub fn ok_response() -> Vec<u8> {
    b"+OK\r\n".to_vec()
}
```

### Priority Levels

| Priority | Feature | Impact | Risk |
|----------|---------|--------|------|
| P0 | opt-single-key-alloc | +5-10% | Low |
| P1 | opt-static-responses | +1-2% | Low |
| P2 | opt-zero-copy-get | +2-3% | Medium |
| P3 | opt-itoa-encode | +1-2% | Low |
| P4 | opt-fxhash-routing | +1% | Low |
| P5 | opt-atoi-parse | +2-3% | Low |

### Benchmarking Workflow

```bash
# Baseline (no optimizations)
cargo build --release
./docker-benchmark/run-benchmarks.sh > baseline.log

# With single optimization
cargo build --release --features opt-single-key-alloc
./docker-benchmark/run-benchmarks.sh > opt-single-key.log

# With all optimizations
cargo build --release --features opt-all
./docker-benchmark/run-benchmarks.sh > opt-all.log

# Compare results
diff baseline.log opt-single-key.log
```

## Consequences

### Positive

- **Measurability**: Each optimization's impact is quantifiable
- **Reversibility**: Can disable problematic optimizations
- **Incrementality**: Add optimizations one at a time
- **CI/CD integration**: Run benchmarks with different feature sets
- **Documentation**: Features document what optimizations exist

### Negative

- **Code complexity**: Conditional compilation adds noise
- **Build matrix**: More combinations to test
- **Binary size**: Unused code paths may increase binary
- **Maintenance**: Must keep feature-gated code in sync

### Risks

- **Feature interaction**: Optimizations may interact unexpectedly
- **Dead code**: Features that are never used
- **Testing gaps**: Not all feature combinations tested

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-03 | Initial ADR created | Need disciplined approach to optimization |
| 2026-01-04 | Features default OFF | Safety first, opt-in to optimizations |
| 2026-01-05 | Add opt-all meta-feature | Convenience for production builds |
| 2026-01-05 | Priority levels P0-P5 | Document expected impact |
| 2026-01-06 | Add itoa/atoi dependencies | Faster integer conversion |
| 2026-01-07 | Benchmark each feature individually | Quantify actual impact |
| 2026-01-08 | Document in BENCHMARK_RESULTS.md | Track optimization history |

## Implementation Status

### Implemented

| Feature | Location | Status |
|---------|----------|--------|
| opt-single-key-alloc | `src/production/sharded_actor.rs` | Reuse key string |
| opt-static-responses | `src/redis/commands.rs` | Pre-allocated OK |
| opt-zero-copy-get | `src/production/sharded_actor.rs` | Avoid data copy |
| opt-itoa-encode | `src/redis/resp.rs` | Fast integer encoding |
| opt-fxhash-routing | `src/production/sharded_actor.rs` | AHash for shard routing (feature named fxhash for historical reasons) |
| opt-atoi-parse | `src/redis/commands.rs` | Fast integer parsing |
| Benchmark tracking | `docker-benchmark/results/` | Timestamped result files; summary in root README.md |

### Validated

- Each feature benchmarked individually
- Combined impact ~10-15% improvement
- No correctness regressions with DST
- All features tested in CI

### Not Yet Implemented

| Feature | Notes |
|---------|-------|
| opt-simd-parsing | SIMD RESP parsing |
| opt-io-uring | io_uring for Linux |
| opt-huge-pages | Huge page allocations |

## References

- [Cargo Features](https://doc.rust-lang.org/cargo/reference/features.html)
- [itoa crate](https://docs.rs/itoa/latest/itoa/)
- [atoi crate](https://docs.rs/atoi/latest/atoi/)
- [FxHash](https://docs.rs/fxhash/latest/fxhash/)
