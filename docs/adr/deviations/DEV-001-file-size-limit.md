# DEV-001: Files Exceeding 500-Line Limit

**Status:** Active
**Related ADR:** ADR-003 (TigerStyle Coding Standards)
**Priority:** Medium
**Created:** 2026-01-25

## Summary

Several files in the codebase exceed the 500-line limit specified in the code quality requirements. These are documented here for tracking and future remediation.

## ADR Intent

ADR-003 and the Quickhouse PR #94 code quality standards specify a 500-line maximum per file to improve maintainability, readability, and code review efficiency.

## Actual Implementation

The following files currently exceed the 500-line limit:

| File | Lines | Category | Notes |
|------|-------|----------|-------|
| `src/redis/commands.rs` | 6025 | Core | Command enum + parser + executor. Tightly coupled, requires careful refactoring |
| `src/replication/lattice.rs` | 1294 | CRDT | CRDT implementations with Kani proofs |
| `src/production/sharded_actor.rs` | 1129 | Production | Sharded actor implementation |
| `src/streaming/compaction.rs` | 1077 | Streaming | Compaction logic |
| `src/production/connection_optimized.rs` | 1021 | Production | Optimized connection handling |
| `src/bin/server_persistent.rs` | 945 | Binary | Persistent server main |
| `src/io/simulation.rs` | 921 | DST | Simulation I/O abstraction |
| `src/streaming/checkpoint.rs` | 916 | Streaming | Checkpoint management |
| `src/replication/crdt_dst.rs` | 853 | DST Tests | CRDT DST tests |
| `src/streaming/compaction_dst.rs` | 806 | DST Tests | Compaction DST tests |

### Successfully Split Files

These files were split to comply with the limit:

1. **`src/replication/state.rs`** (was 776 lines)
   - Split into: `state/mod.rs`, `state/tests.rs`, `state/hash_tests.rs`, `state/hincrby_tests.rs`, `state/conditional_tests.rs`, `state/type_mismatch_tests.rs`
   - All files now under 327 lines

2. **`src/redis/data.rs`** (was 2,496 lines)
   - Split into: `data/mod.rs`, `data/skiplist.rs`, `data/sds.rs`, `data/value.rs`, `data/list.rs`, `data/set.rs`, `data/hash.rs`, `data/sorted_set.rs`
   - Most files under 500 lines (skiplist.rs: 525, sorted_set.rs: 502)

3. **`src/redis/tests.rs`** (was 2,243 lines)
   - Split into 11 test files in `tests/` directory
   - All files under 306 lines

## Rationale

- **commands.rs**: The Command enum, parser, and executor are tightly coupled. Splitting requires careful refactoring to avoid circular dependencies and maintain the clean API surface.
- **DST test files**: These are comprehensive test suites that benefit from being in single files for readability of test scenarios.
- **Streaming/Production**: These modules have complex state machines that are easier to understand in single files.

## Impact

- Code review efficiency is reduced for larger files
- Navigation and understanding is harder
- IDE performance may be affected

## Resolution Path

### Phase 1 (Completed)
- ✅ Split `src/replication/state.rs` tests
- ✅ Split `src/redis/data.rs` into modules
- ✅ Split `src/redis/tests.rs` into modules

### Phase 2 (Future)
- [ ] Split `src/redis/commands.rs` using delegation pattern:
  - Extract `Command` enum to `command.rs`
  - Extract parser to `parser.rs` (may need sub-splits)
  - Extract executor using trait-based delegation
- [ ] Split `src/streaming/compaction.rs`
- [ ] Split `src/production/sharded_actor.rs`

### Phase 3 (Future)
- [ ] Evaluate remaining files for splitting opportunities
- [ ] Consider using `include!` macro for very large match statements

## Decision Log

| Date | Decision | By |
|------|----------|-----|
| 2026-01-25 | Initial deviation documented | Claude |
| 2026-01-25 | Completed Phase 1 splits (state, data, tests) | Claude |
