# Architecture Decision Records

This directory contains Architecture Decision Records (ADRs) for redis-rust.

> **See also:** [EVOLUTION.md](./EVOLUTION.md) - How we track architectural change through characteristics, gaps, and deviations.

## What is an ADR?

An ADR is a **living decision log** that captures architectural decisions and how they evolve over time. Unlike static documentation, ADRs are actively maintained as we learn and make new decisions.

ADRs help teams:

- Understand why decisions were made
- Track how decisions evolved based on learnings
- Avoid revisiting the same discussions
- Onboard new team members quickly
- Maintain consistency across the codebase

## ADRs as Decision Logs

**Each ADR contains a Decision Log section** that tracks the evolution of that architectural area. When you make a decision related to an existing ADR:

1. **Add an entry** to the Decision Log table with date, decision, and rationale
2. **Update Implementation Status** if the decision affects what's built
3. **Update the main sections** if the decision significantly changes the approach

### When to Add a Decision Log Entry

Add an entry when you:
- Choose between alternative implementations
- Discover constraints that affect the architecture
- Defer or reject a planned feature
- Change approach based on learnings
- Make trade-offs during implementation
- Integrate with external systems in a specific way

### Example Decision Log Entry

```markdown
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-09 | Use tokio::mpsc instead of crossbeam channels | Better async integration, consistent with rest of codebase |
```

## ADR Index

| ADR | Title | Status | Summary |
|-----|-------|--------|---------|
| [000](./000-template.md) | Template | - | Template for new ADRs |
| [001](./001-simulation-first-development.md) | Simulation-First Development (DST) | Accepted | FoundationDB/TigerBeetle-style deterministic testing |
| [002](./002-actor-per-shard-architecture.md) | Actor-per-Shard Architecture | Accepted | Lock-free message passing for sharded state |
| [003](./003-tigerstyle-coding-standards.md) | TigerStyle Coding Standards | Accepted | Safety-first engineering with assertions |
| [004](./004-anna-kvs-crdt-replication.md) | Anna KVS CRDT Replication | Accepted | Eventual/causal consistency via CRDTs |
| [005](./005-streaming-persistence.md) | Streaming Persistence to Object Store | Accepted | Cloud-native S3 persistence, not RDB/AOF |
| [006](./006-zero-copy-resp-parser.md) | Zero-Copy RESP Parser | Accepted | High-performance protocol parsing |
| [007](./007-feature-flag-optimizations.md) | Feature Flag Optimization Strategy | Accepted | Incremental, measurable performance tuning |
| [008](./008-docker-benchmark-methodology.md) | Docker-Only Benchmark Methodology | Accepted | Fair, reproducible performance comparisons |
| [009](./009-security-tls-acl.md) | Security - TLS and ACL | Accepted | Optional TLS encryption and Redis 6.0+ ACL |
| [010](./010-wal-hybrid-persistence.md) | WAL + Streaming Hybrid Persistence | Accepted | Local WAL with group commit for zero-RPO durability |

## Status Definitions

- **Proposed**: Under discussion, not yet approved
- **Accepted**: Approved and guides implementation
- **Deprecated**: No longer relevant, kept for historical context
- **Superseded**: Replaced by a newer ADR

## Creating a New ADR

1. Copy `000-template.md` to `NNN-descriptive-title.md`
2. Fill in all sections
3. Submit for review
4. Update this README with the new entry

## Key Architectural Principles

These ADRs reflect our core architectural principles:

1. **Simulation-first development**: If you can't simulate it, you can't test it properly
2. **Actor isolation**: Message passing over shared mutable state
3. **TigerStyle safety**: Assertions, checked arithmetic, explicit errors
4. **Eventual consistency**: CRDT-based replication for coordination-free scaling
5. **Cloud-native persistence**: Object store streaming, not traditional AOF/RDB
6. **Zero-copy performance**: Minimize allocations in hot paths
7. **Measurable optimization**: Feature flags for A/B testing performance changes
8. **Reproducible benchmarks**: Docker containers for fair comparisons

## Project Vision

redis-rust aims to be a research-grade, Redis-compatible cache server that demonstrates:

- How to build reliable distributed systems using simulation testing
- Actor-based architecture for lock-free concurrency
- CRDT-based replication for coordination-free scaling
- Cloud-native persistence patterns (object store vs local disk)
- Modern Rust systems programming practices (TigerStyle)

This is an **experimental research project**, not a production Redis replacement.
