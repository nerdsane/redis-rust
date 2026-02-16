# Architecture Gaps

This directory contains **Gap** documents - lightweight pre-ADR records of design limitations discovered through DST, production behavior, or research.

## What is a Gap?

A gap captures a **design limitation** that may need architectural attention. It's lighter weight than an ADR - more of a "discovery log" that may evolve into a formal ADR if the solution warrants it.

## When to Create a Gap

Create a gap when you discover:
- A design limitation exposed by DST or production testing
- A pattern used by other systems (TigerBeetle, FoundationDB) that we're missing
- A recurring problem that needs architectural attention
- A scalability concern found during load testing

## Gap Template

```markdown
# GAP-NNN: Title

**Status:** Open | Investigating | ADR-Drafted | Closed | Won't Fix
**Severity:** Critical | High | Medium | Low
**Discovered:** YYYY-MM-DD
**DST Seeds:** (if applicable)

## Summary
One paragraph describing the gap.

## Evidence
How was this discovered? Link to DST failures, metrics, or observations.

## Impact
What does this affect? Performance, correctness, scalability?

## Potential Solutions
Ideas for addressing this gap.

## Related
- Links to related ADRs, code, or external references
```

## Gap Lifecycle

```
Discovered → Open → Investigating → ADR-Drafted → Closed
                         ↓
                   (Won't Fix) → Closed
```

- **Open**: Gap identified, not yet analyzed
- **Investigating**: Actively researching solutions
- **ADR-Drafted**: Solution designed, ADR in progress
- **Closed**: Either fixed or formally documented as deviation
- **Won't Fix**: Accepted as limitation, becomes a deviation

## Current Gaps

| Gap | Title | Status | Severity |
|-----|-------|--------|----------|
| - | (None yet) | - | - |

## Gap vs Deviation vs ADR

| Document | Purpose | When |
|----------|---------|------|
| **Gap** | Record a discovered limitation | Something isn't working as expected |
| **Deviation** | Document intentional divergence | We chose not to follow an ADR |
| **ADR** | Record architectural decision | We designed a solution |

A gap may evolve into either:
- An **ADR** (if we design and implement a fix)
- A **Deviation** (if we accept the limitation)

## Examples from Other Projects

**FoundationDB Gap Pattern:**
> "We discovered that during network partitions longer than X seconds, the coordinator election protocol can temporarily stall. This was found via simulation seed 12345."

**TigerBeetle Gap Pattern:**
> "Storage engine assumes sequential disk writes. Found that some cloud providers reorder writes, causing corruption. DST seed 778665."
