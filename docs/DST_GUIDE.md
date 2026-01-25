# Deterministic Simulation Testing (DST) Guide

redis-rust implements state-of-the-art Deterministic Simulation Testing inspired by FoundationDB and TigerBeetle. This guide covers our comprehensive verification infrastructure.

## Overview

DST allows us to find bugs that would take years to manifest in production by:
- **Deterministic replay**: Same seed = same execution = reproducible bugs
- **Accelerated time**: Test hours of real-world scenarios in seconds
- **Fault injection**: Simulate network partitions, disk failures, message drops
- **Exhaustive exploration**: Model checking for critical invariants

## Architecture

```
                          Verification Pyramid
                                 │
    ┌────────────────────────────┼────────────────────────────┐
    │                            │                            │
    ▼                            ▼                            ▼
 TLA+ Specs                  DST Tests                  Production
 (Formal model)              (Simulation)              (Monitoring)
    │                            │                            │
    ▼                            ▼                            ▼
 Stateright/Kani            35+ Fault Types            Datadog Alerts
 (Exhaustive)               (Realistic)                (Observability)
```

## Fault Injection System

### Named Fault Types (35+)

Our fault injection system defines specific, named faults that map to real-world failure modes:

| Category | Fault Name | Description |
|----------|------------|-------------|
| **Network** | `NETWORK_PARTITION` | Complete network isolation |
| | `NETWORK_DELAY` | Latency injection (10ms-5s) |
| | `NETWORK_DROP` | Random packet loss |
| | `NETWORK_DUPLICATE` | Message duplication |
| | `NETWORK_REORDER` | Out-of-order delivery |
| **Storage** | `DISK_SLOW` | I/O latency spikes |
| | `DISK_FAIL` | Storage operation failures |
| | `DISK_CORRUPT` | Silent data corruption |
| | `DISK_FULL` | No space left on device |
| **Process** | `PROCESS_CRASH` | Abrupt termination |
| | `PROCESS_PAUSE` | GC-like pauses |
| | `PROCESS_SLOW` | CPU contention simulation |
| **Clock** | `CLOCK_SKEW` | Time drift between nodes |
| | `CLOCK_JUMP` | Sudden time changes |
| **Memory** | `OOM_PRESSURE` | Memory allocation failures |

### Fault Configuration

```rust
use crate::simulator::FaultConfig;

// Create fault configuration
let fault_config = FaultConfig::builder()
    .with_fault("NETWORK_PARTITION", 0.01)  // 1% chance per operation
    .with_fault("NETWORK_DELAY", 0.05)
    .with_fault("DISK_SLOW", 0.02)
    .with_fault("PROCESS_PAUSE", 0.01)
    .build();

// Or use presets
let chaos_config = FaultConfig::chaos();      // High fault rate for stress testing
let realistic_config = FaultConfig::production_like();  // Based on real failure data
```

## Workload Generation

### Zipfian Distribution

Real workloads follow Zipfian (power-law) distributions, not uniform:

```rust
use crate::simulator::KeyDistribution;

// GOOD: Realistic hot/cold key pattern
// With skew=1.0, top 10 keys get ~40% of traffic
let distribution = KeyDistribution::Zipfian {
    num_keys: 1000,
    skew: 1.0,
};

// BAD: Uniform distribution (unrealistic)
let distribution = KeyDistribution::Uniform { num_keys: 1000 };
```

### Mixed Workloads

```rust
let workload = Workload::mixed()
    .with_operation(Operation::Set, 0.30)    // 30% writes
    .with_operation(Operation::Get, 0.60)    // 60% reads
    .with_operation(Operation::Delete, 0.05) // 5% deletes
    .with_operation(Operation::Scan, 0.05)   // 5% scans
    .with_key_distribution(KeyDistribution::Zipfian { num_keys: 10_000, skew: 0.99 });
```

## Writing DST Tests

### Basic Structure

```rust
#[test]
fn test_replication_under_partitions() {
    // Run with multiple seeds for coverage
    for seed in 0..50 {
        let mut harness = SimulationHarness::new(seed);

        // Configure nodes
        harness.add_node("node-1");
        harness.add_node("node-2");
        harness.add_node("node-3");

        // Enable fault injection
        harness.enable_faults(FaultConfig::chaos());

        // Run scenario
        harness.run_scenario(|ctx| async move {
            // Perform operations
            ctx.node("node-1").set("key", "value").await?;

            // Inject specific fault
            ctx.inject_fault("node-2", Fault::NetworkPartition);

            // Continue operations
            ctx.node("node-3").get("key").await?;

            // Heal partition
            ctx.heal_fault("node-2", Fault::NetworkPartition);

            // Verify convergence
            ctx.wait_for_convergence(Duration::from_secs(10)).await?;

            Ok(())
        });

        // Verify invariants after scenario
        harness.verify_invariants();
    }
}
```

### Multi-Node Simulation

```rust
#[test]
fn test_gossip_protocol() {
    for seed in 0..100 {
        let harness = SimulationHarness::new(seed)
            .with_nodes(5)
            .with_faults(FaultConfig::builder()
                .with_fault("NETWORK_DELAY", 0.1)
                .with_fault("MESSAGE_DROP", 0.05)
                .build());

        harness.run_scenario(|ctx| async move {
            // Originate update at node 0
            ctx.node(0).set("gossip-key", "gossip-value").await?;

            // Allow gossip propagation with time advancement
            ctx.advance_time(Duration::from_secs(30)).await;

            // All nodes should have the value
            for node_id in 0..5 {
                let value = ctx.node(node_id).get("gossip-key").await?;
                assert_eq!(value, Some("gossip-value".into()));
            }

            Ok(())
        });
    }
}
```

## Time Control

### Virtual Time

DST uses virtual time that can be controlled programmatically:

```rust
use crate::io::VirtualTime;

// Get current virtual time
let now = ctx.virtual_time().now();

// Advance time (fast-forward)
ctx.advance_time(Duration::from_secs(60)).await;

// Test timeout scenarios
ctx.set_time_scale(100.0);  // 100x speedup

// Test specific time
ctx.set_virtual_time(Instant::from_secs(1000));
```

### Timeout Testing

```rust
#[test]
fn test_leader_election_timeout() {
    let mut harness = SimulationHarness::new(42);

    harness.run_scenario(|ctx| async move {
        // Partition leader
        ctx.inject_fault("leader", Fault::NetworkPartition);

        // Fast-forward past election timeout
        ctx.advance_time(Duration::from_secs(30)).await;

        // Verify new leader elected
        let new_leader = ctx.get_leader().await;
        assert_ne!(new_leader, "leader");

        Ok(())
    });
}
```

## Model Checking with Stateright

For critical invariants, we use exhaustive state exploration:

```rust
use stateright::*;

#[derive(Clone, Debug, Hash)]
struct ReplicationModel {
    nodes: Vec<NodeState>,
    network: Vec<Message>,
}

impl Model for ReplicationModel {
    type State = Self;
    type Action = ReplicationAction;

    fn init_states(&self) -> Vec<Self::State> {
        vec![Self::initial()]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        // Generate all possible actions
        for node in 0..state.nodes.len() {
            actions.push(ReplicationAction::Write { node, key: "k", value: "v" });
            actions.push(ReplicationAction::Sync { from: node, to: (node + 1) % 3 });
        }
    }

    fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
        // Apply action and return new state
        Some(state.apply(action))
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("convergence", |_, state| {
                state.is_converged()
            }),
            Property::always("no_lost_writes", |_, state| {
                state.all_acked_writes_visible()
            }),
        ]
    }
}

#[test]
#[ignore]  // Exhaustive - run with: cargo test stateright -- --ignored
fn stateright_replication() {
    ReplicationModel::new()
        .checker()
        .threads(num_cpus::get())
        .spawn_dfs()
        .join()
        .assert_properties();
}
```

## Kani Bounded Proofs

For mathematical properties, we use Kani for bounded model checking:

```rust
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn verify_lww_merge_commutative() {
        let a: i64 = kani::any();
        let b: i64 = kani::any();
        let ts_a: u64 = kani::any();
        let ts_b: u64 = kani::any();

        let lww_a = LWWRegister::new(a, ts_a);
        let lww_b = LWWRegister::new(b, ts_b);

        // Merge is commutative
        let result_ab = lww_a.merge(&lww_b);
        let result_ba = lww_b.merge(&lww_a);

        assert_eq!(result_ab, result_ba);
    }

    #[kani::proof]
    fn verify_lamport_clock_monotonic() {
        let mut clock = LamportClock::new();
        let initial = clock.now();

        // Any operation must increase the clock
        let next = clock.tick();
        assert!(next > initial);

        // Receive must be >= max(local, remote)
        let remote: u64 = kani::any();
        let after_receive = clock.receive(remote);
        assert!(after_receive > next);
        assert!(after_receive > remote);
    }
}
```

Run Kani proofs:
```bash
cargo kani --harness verify_lww_merge_commutative
cargo kani --harness verify_lamport_clock_monotonic
```

## Maelstrom Integration

For Jepsen-style distributed systems testing:

```bash
# Build the Maelstrom node binary
cargo build --release --bin maelstrom-kv-replicated

# Run linearizability test
./maelstrom/maelstrom test -w lin-kv \
    --bin ./target/release/maelstrom-kv-replicated \
    --node-count 3 \
    --time-limit 60 \
    --rate 100 \
    --concurrency 10

# Run with nemesis (fault injection)
./maelstrom/maelstrom test -w lin-kv \
    --bin ./target/release/maelstrom-kv-replicated \
    --node-count 5 \
    --time-limit 120 \
    --rate 50 \
    --nemesis partition
```

## TLA+ Specifications

Our TLA+ specs define the formal model that DST tests verify:

| Specification | Location | Invariants |
|---------------|----------|------------|
| Replication Convergence | `specs/tla/ReplicationConvergence.tla` | CRDT merge properties |
| Gossip Protocol | `specs/tla/GossipProtocol.tla` | Delivery guarantees |
| Streaming Persistence | `specs/tla/StreamingPersistence.tla` | Durability, ordering |
| Anti-Entropy | `specs/tla/AntiEntropy.tla` | Merkle tree consistency |

## Invariant Mappings

The `invariant_mappings.yaml` file connects formal specs to code:

```yaml
invariants:
  CRDT_MERGE_COMMUTATIVE:
    tla_spec: ReplicationConvergence.tla
    code_location: src/replication/lattice.rs:42
    tests:
      - stateright_replication
      - kani::verify_lww_merge_commutative
    monitors:
      - dd.redis_rust.crdt.merge_conflicts

  LAMPORT_MONOTONIC:
    tla_spec: ReplicationConvergence.tla
    code_location: src/replication/clock.rs:15
    tests:
      - kani::verify_lamport_clock_monotonic
      - test_lamport_clock_ordering
```

## Running DST Tests

### Quick Smoke Test
```bash
# Single seed, fast feedback
cargo test streaming_dst --release -- --nocapture
```

### Comprehensive Coverage
```bash
# Multiple seeds for better coverage
for seed in $(seq 0 99); do
    DST_SEED=$seed cargo test streaming_dst --release
done
```

### Targeted Fault Testing
```bash
# Test specific fault type
DST_FAULTS=NETWORK_PARTITION,DISK_SLOW cargo test replication_dst --release
```

### Full Verification Suite
```bash
# Run everything (takes ~30 minutes)
./scripts/local-ci.sh

# Or individual components:
cargo test stateright -- --ignored --nocapture
cargo kani --harness verify_lww_merge_commutative
./maelstrom/maelstrom test -w lin-kv --bin ./target/release/maelstrom-kv-replicated
```

## Debugging Failed Seeds

When a test fails, the seed is printed for reproduction:

```
test streaming_dst ... FAILED
DST seed: 12345
Fault sequence: [NETWORK_PARTITION@t=100, DISK_SLOW@t=250]
```

Reproduce:
```bash
DST_SEED=12345 cargo test streaming_dst --release -- --nocapture
```

## Best Practices

1. **Always run multiple seeds** - Minimum 10, ideally 50+
2. **Use Zipfian distributions** - Uniform is unrealistic
3. **Verify invariants after every mutation** - Catch bugs immediately
4. **Save failing seeds** - Add them to regression tests
5. **Start with chaos, narrow down** - Begin with high fault rates
6. **Test recovery, not just failures** - Verify system heals after faults

## File Locations

| Component | Location |
|-----------|----------|
| Simulation harness | `src/simulator/` |
| Fault injection | `src/buggify/` |
| Simulated I/O | `src/io/` |
| Stateright models | `src/stateright/` |
| TLA+ specs | `specs/tla/` |
| Invariant mappings | `invariant_mappings.yaml` |
| DST integration tests | `src/simulator/dst_integration.rs` |
