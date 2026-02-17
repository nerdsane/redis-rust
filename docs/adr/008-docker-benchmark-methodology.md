# ADR-008: Docker-Only Benchmark Methodology

## Status

Accepted

## Context

Performance claims are meaningless without reproducible methodology. Common benchmarking mistakes include:

1. **Unfair comparisons**: Different hardware, OS, or configuration
2. **Cherry-picked results**: Showing best case, hiding worst case
3. **Unreproducible**: "Trust me, it was faster"
4. **Apples-to-oranges**: Comparing pipelined vs non-pipelined

We need a benchmark methodology that is:
- **Fair**: Both servers under identical conditions
- **Reproducible**: Anyone can run the same benchmarks
- **Comprehensive**: Multiple scenarios (pipelined, non-pipelined)
- **Documented**: Clear methodology in results

## Decision

We will use **Docker containers** as the **only valid method** for Redis vs Rust performance comparisons:

### Docker Configuration

```yaml
# Both containers get identical resources
services:
  redis:
    image: redis:7.4-alpine   # Primary comparison target
    cpus: 2
    memory: 1G

  rust-redis:
    build: .
    cpus: 2
    memory: 1G

# Redis 8.0 comparison available via docker-compose.redis8.yml
```

### Benchmark Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| CPU Limit | 2 cores | Realistic server constraint |
| Memory Limit | 1GB | Realistic server constraint |
| Network | Host networking | Eliminates Docker NAT overhead |
| Requests | 100,000 | Statistically significant |
| Clients | 50 concurrent | Realistic load |
| Data Size | 64 bytes | Standard benchmark size |
| Pipeline depths | 1, 16 | Tests non-pipelined and pipelined patterns (P=64 available in run-detailed-benchmarks.sh) |

### Benchmark Scripts

```bash
# Run fair comparison
cd docker-benchmark
./run-benchmarks.sh        # Redis 7.4 vs Rust
./run-redis8-comparison.sh # Redis 7.4 vs 8.0 vs Rust
./run-persistent-benchmarks.sh # With persistence
```

### Results Format

```markdown
## [Date] - [Change Description]

### Test Configuration
- **Method**: Docker benchmarks (docker-benchmark/run-benchmarks.sh)
- **Docker**: [version]
- **Host OS**: [OS version]
- **CPU Limit**: 2 cores per container
- **Memory Limit**: 1GB per container

### Results (Docker - Fair Comparison)
| Operation | Redis 7.4 | Rust | Relative |
|-----------|-----------|------|----------|
| SET (P=1) | X req/s   | Y req/s | Z% |
| GET (P=1) | X req/s   | Y req/s | Z% |
```

### What NOT to Use

```bash
# DON'T use these for Redis vs Rust comparisons:
cargo run --release --bin quick_benchmark  # Local only
cargo run --release --bin benchmark        # Unfair conditions
```

These local benchmarks are valid for:
- Quick smoke tests during development
- Profiling and optimization work
- Regression detection between commits

But they **MUST NOT** be used for Redis vs Rust performance claims.

## Consequences

### Positive

- **Credibility**: Results are reproducible by anyone
- **Fairness**: Identical conditions eliminate bias
- **Transparency**: Methodology is documented
- **Automation**: Scripts make benchmarks easy to run

### Negative

- **Setup overhead**: Docker required to run benchmarks
- **Slower iteration**: Docker builds take longer than local
- **Resource limits**: May hide optimization opportunities
- **Platform specifics**: Docker behavior varies by OS

### Risks

- **Docker overhead**: May not reflect bare-metal performance
- **Version drift**: Docker/Redis versions change over time
- **Network mode**: Host networking may not work everywhere

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-03 | Initial ADR created | Need reproducible benchmark methodology |
| 2026-01-03 | Require Docker for comparisons | Only fair way to compare |
| 2026-01-04 | Use host networking | Eliminate NAT overhead |
| 2026-01-05 | Document in CLAUDE.md | Ensure team follows methodology |
| 2026-01-05 | Add Redis 8.0 comparison | Track against latest Redis |
| 2026-01-06 | Require multiple pipeline depths | Different patterns tell different stories |
| 2026-01-07 | Add results directory | Track benchmark history |
| 2026-01-08 | Run 3x and take median | Reduce variance in results |

## Implementation Status

### Implemented

| Component | Location | Status |
|-----------|----------|--------|
| Docker configs | `docker-benchmark/` | Redis 7.4, 8.0, Rust containers |
| run-benchmarks.sh | `docker-benchmark/` | Automated comparison script |
| run-redis8-comparison.sh | `docker-benchmark/` | Three-way comparison |
| run-persistent-benchmarks.sh | `docker-benchmark/` | Persistence benchmark |
| Results tracking | `docker-benchmark/results/` | Timestamped historical results |
| Results summary | Root `README.md` | Current results inline in README |

### Validated

- Benchmarks run successfully on Linux
- Results are reproducible across runs
- Docker resource limits enforced correctly
- Host networking eliminates NAT overhead

### Not Yet Implemented

| Component | Notes |
|-----------|-------|
| CI/CD integration | Benchmarks not in CI pipeline |
| Multi-platform | Only tested on Linux |
| Automated reporting | Manual update of BENCHMARK_RESULTS.md |

## References

- [redis-benchmark documentation](https://redis.io/docs/management/optimization/benchmarks/)
- [Docker resource constraints](https://docs.docker.com/config/containers/resource_constraints/)
- [How to benchmark correctly](https://www.brendangregg.com/blog/2014-05-02/compilers-benchmarking.html)
