# Architecture Evolution

This document defines how redis-rust tracks architectural change through three complementary lenses.

## The Three Lenses

```
                        Architecture Evolution
                                 │
        ┌────────────────────────┼────────────────────────┐
        │                        │                        │
        ▼                        ▼                        ▼
  Characteristics              Gaps                  Deviations
   (Proactive)              (Reactive)              (Pragmatic)
        │                        │                        │
  "What we need"          "What we learned"      "What we accepted"
        │                        │                        │
  From requirements       From production         From trade-offs
```

| Lens | Question | Trigger | Outcome |
|------|----------|---------|---------|
| **Characteristics** | What capabilities must we have? | Product requirements, competitive analysis | Feature roadmap, ADR targets |
| **Gaps** | What limitations have we discovered? | Incidents, scale tests, code reviews | Potential ADRs, system improvements |
| **Deviations** | Where did we intentionally diverge? | Implementation trade-offs, PoC scope | Documented tech debt, migration plans |

## Characteristics (Proactive)

**Location:** Embedded in relevant ADRs (particularly [ADR-001](./001-simulation-first-development.md) for DST, [ADR-004](./004-anna-kvs-crdt-replication.md) for replication)

**Purpose:** Track implementation status of key distributed cache characteristics.

**Status Legend:**
- ✅ Implemented - Production ready
- 🟡 Partial - Some aspects implemented, gaps remain
- 🟢 Implicit - Achieved as side effect
- ⏳ Proposed - ADR exists but not implemented
- ❌ Deviation/Not Planned - Gap or intentional omission

### Key Characteristics for redis-rust

| # | Characteristic | Status | ADR | Notes |
|---|----------------|--------|-----|-------|
| 1 | CRDT-based replication | ✅ | ADR-004 | LWW registers, G-Counters, OR-Sets |
| 2 | Gossip protocol | ✅ | ADR-004 | Selective routing, anti-entropy |
| 3 | Deterministic simulation | ✅ | ADR-001 | Multi-seed DST, buggify |
| 4 | Actor isolation | ✅ | ADR-002 | Message passing, no shared state |
| 5 | Streaming persistence | ✅ | ADR-005 | S3/object store, delta encoding |
| 6 | TigerStyle assertions | ✅ | ADR-003 | verify_invariants() pattern |
| 7 | TLA+ specifications | ⏳ | - | Formal specs for protocols |
| 8 | Stateright model checking | ⏳ | - | Exhaustive state exploration |
| 9 | Kani bounded proofs | ⏳ | - | Bounded verification |
| 10 | Linearizability (single-node) | ✅ | ADR-001 | Maelstrom verified |
| 11 | Linearizability (multi-node) | ❌ | - | By design: eventual consistency |

## Gaps (Reactive)

**Location:** [gaps/](./gaps/)

**Purpose:** Capture design limitations discovered through production behavior, DST runs, or research. Lightweight pre-ADR documents that may evolve into formal ADRs.

**Lifecycle:**
```
Discovered → Open → Investigating → ADR-Drafted → Closed
                         ↓
                   (Won't Fix) → Closed
```

**When to Create:**
- Design limitation exposed by DST or production
- Pattern used by other systems that we're missing
- Recurring problem needing architectural attention

**Current Gaps:**

| Gap | Title | Status | Severity |
|-----|-------|--------|----------|
| - | (None yet) | - | - |

## Deviations (Pragmatic)

**Location:** [deviations/](./deviations/)

**Purpose:** Document where actual implementation intentionally differs from ADR intent. These are known, accepted trade-offs - not bugs.

**When to Create:**
- Implementation takes a different approach than ADR described
- PoC simplification that will need future work
- Architectural compromise due to time/resource constraints

**Current Deviations:**

| Deviation | Title | Related ADR | Priority |
|-----------|-------|-------------|----------|
| - | (None yet) | - | - |

## Relationships

### Gaps → Characteristics
A gap may provide evidence for a characteristic's partial status.

**Example:** If we discover CRDT merge is slow under high contention, that gap explains why Characteristic #1 might need refinement.

### Gaps → ADRs
A gap may evolve into a formal ADR when the solution is designed.

**Example:** Gap about missing TLA+ specs → ADR-010 (Formal Verification).

### Gaps → Deviations
A gap may become a deviation if we decide to accept the limitation.

**Example:** If we decide some limitation is acceptable at current scale, the gap becomes a documented deviation.

### Characteristics → Deviations
A characteristic marked ❌ should have a corresponding deviation explaining why.

**Example:** Characteristic #11 (Multi-node linearizability) ❌ → documented as intentional design choice for eventual consistency.

## Decision Flow

```
Problem Discovered
       │
       ▼
  Is it a known requirement?
       │
  ┌────┴────┐
  │ Yes     │ No
  ▼         ▼
Update    Create Gap
Characteristic   │
Status           ▼
            Can we fix it?
                 │
           ┌─────┴─────┐
           │ Yes       │ No (or not now)
           ▼           ▼
      Draft ADR    Create Deviation
      (close gap)  (document trade-off)
```

## Maintenance

| Document Type | Review Cadence | Owner |
|---------------|----------------|-------|
| Characteristics | Monthly (with DST runs) | Engineering |
| Gaps | After DST failures, scale tests | Engineering |
| Deviations | Before major releases | Tech Lead |

## References

- [ADR Index](./README.md)
- [ADR-001: Simulation-First Development](./001-simulation-first-development.md)
- [ADR-004: Anna KVS CRDT Replication](./004-anna-kvs-crdt-replication.md)
- [Gaps Directory](./gaps/README.md)
- [Deviations Directory](./deviations/README.md)
