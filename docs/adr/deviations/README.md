# Architecture Deviations

This directory contains **Deviation** documents - records of intentional divergence from ADR specifications or architectural intent.

## What is a Deviation?

A deviation documents where the **actual implementation intentionally differs** from what an ADR describes. These are known, accepted trade-offs - not bugs or oversights.

## When to Create a Deviation

Create a deviation when:
- Implementation takes a different approach than the ADR described
- A PoC simplification will need future work to match ADR intent
- Time/resource constraints forced an architectural compromise
- A gap was closed with "Won't Fix" - the limitation is accepted

## Deviation Template

```markdown
# DEV-NNN: Title

**Status:** Active | Resolved | Accepted
**Related ADR:** ADR-NNN
**Priority:** Critical | High | Medium | Low | Acceptable
**Created:** YYYY-MM-DD

## Summary
One paragraph describing the deviation.

## ADR Intent
What the ADR says should happen.

## Actual Implementation
What we actually built and why.

## Rationale
Why this deviation was acceptable.

## Impact
What does this affect? Any risks?

## Resolution Path
How and when might this be resolved? Or is it permanently acceptable?

## Decision Log

| Date | Decision | By |
|------|----------|-----|
| YYYY-MM-DD | Initial deviation documented | Name |
```

## Deviation Lifecycle

```
Identified → Active → (Resolved | Accepted)
```

- **Active**: Deviation exists, may be addressed later
- **Resolved**: Deviation was addressed, implementation now matches ADR
- **Accepted**: Deviation is permanent, considered acceptable

## Current Deviations

| Deviation | Title | Related ADR | Priority |
|-----------|-------|-------------|----------|
| [DEV-001](DEV-001-file-size-limit.md) | Files Exceeding 500-Line Limit | ADR-003 | Medium |

## Priority Definitions

- **Critical**: Must be addressed before production use
- **High**: Should be addressed in near-term
- **Medium**: Should be addressed eventually
- **Low**: Nice to have, not blocking
- **Acceptable**: Permanent deviation, documented trade-off

## Deviation vs Gap vs ADR

| Document | Purpose | Tone |
|----------|---------|------|
| **Gap** | "We found a problem" | Discovery |
| **Deviation** | "We chose differently" | Intentional |
| **ADR** | "This is our design" | Prescriptive |

## Examples

### Good Deviation Documentation

```markdown
# DEV-001: Simplified CRDT Merge for PoC

**Related ADR:** ADR-004 (CRDT Replication)
**Priority:** Medium

## ADR Intent
ADR-004 specifies using optimized CRDT merge with delta compression.

## Actual Implementation
Current implementation uses naive full-state merge without delta compression.

## Rationale
Delta compression adds complexity. For PoC with <10 nodes, full-state merge is acceptable and simpler to verify in DST.

## Resolution Path
Implement delta compression when scaling beyond 10 nodes or when gossip bandwidth becomes a bottleneck.
```

### When NOT to Create a Deviation

- **Bugs**: If it's unintentional, it's a bug, not a deviation
- **Missing features**: If it's just not built yet, it's not a deviation
- **Minor differences**: If the spirit of the ADR is preserved, no need to document
