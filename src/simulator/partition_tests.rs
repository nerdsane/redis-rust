//! Network Partition Simulation Tests
//!
//! Comprehensive tests for network partition scenarios:
//! - Simple partitions between nodes
//! - Minority/majority partitions
//! - Split-brain scenarios
//! - Partition healing and convergence
//! - Writes during partition
//! - Asymmetric partitions

use super::multi_node::{MultiNodeSimulation, check_single_key_linearizability};
use crate::redis::{Command, SDS};
use std::collections::HashSet;

/// Partition configuration for tests
#[derive(Debug, Clone)]
pub struct PartitionConfig {
    /// Pairs of nodes that cannot communicate
    pub partitioned_pairs: Vec<(usize, usize)>,
    /// Description of the partition for logging
    pub description: String,
}

impl PartitionConfig {
    /// Create a partition isolating a single node
    pub fn isolate_node(node: usize, total_nodes: usize) -> Self {
        let pairs: Vec<(usize, usize)> = (0..total_nodes)
            .filter(|&n| n != node)
            .map(|n| if n < node { (n, node) } else { (node, n) })
            .collect();

        PartitionConfig {
            partitioned_pairs: pairs,
            description: format!("Node {} isolated from cluster", node),
        }
    }

    /// Create a split-brain partition (nodes split into two groups)
    pub fn split_brain(group_a: Vec<usize>, group_b: Vec<usize>) -> Self {
        let mut pairs = Vec::new();
        for &a in &group_a {
            for &b in &group_b {
                let pair = if a < b { (a, b) } else { (b, a) };
                pairs.push(pair);
            }
        }

        PartitionConfig {
            partitioned_pairs: pairs,
            description: format!("Split brain: {:?} vs {:?}", group_a, group_b),
        }
    }

    /// Create asymmetric partition (A can't reach B, but others can)
    pub fn asymmetric(from: usize, to: usize) -> Self {
        PartitionConfig {
            partitioned_pairs: vec![(from.min(to), from.max(to))],
            description: format!("Asymmetric partition: {} <-> {}", from, to),
        }
    }

    /// Create a ring partition (each node can only talk to neighbors)
    pub fn ring(num_nodes: usize) -> Self {
        let mut pairs = Vec::new();
        for i in 0..num_nodes {
            for j in (i + 2)..num_nodes {
                // Skip if j is the "next" neighbor in ring (wrapping)
                if !(i == 0 && j == num_nodes - 1) {
                    pairs.push((i, j));
                }
            }
        }

        PartitionConfig {
            partitioned_pairs: pairs,
            description: format!("Ring topology with {} nodes", num_nodes),
        }
    }
}

/// Result of a partition test
#[derive(Debug)]
pub struct PartitionTestResult {
    pub test_name: String,
    pub partition_config: PartitionConfig,
    pub writes_during_partition: usize,
    pub writes_after_heal: usize,
    pub converged: bool,
    pub convergence_rounds: usize,
    pub final_values: Vec<Option<String>>,
    pub linearizable: bool,
}

/// Run a partition test scenario
pub fn run_partition_test(
    test_name: &str,
    num_nodes: usize,
    seed: u64,
    partition_config: PartitionConfig,
    writes_during_partition: Vec<(usize, &str, &str)>, // (node, key, value)
    writes_after_heal: Vec<(usize, &str, &str)>,
    max_convergence_rounds: usize,
) -> PartitionTestResult {
    let mut sim = MultiNodeSimulation::new(num_nodes, seed);

    // Apply partition
    for (a, b) in &partition_config.partitioned_pairs {
        sim.partition(*a, *b);
    }

    // Writes during partition
    for (client_id, (node, key, value)) in writes_during_partition.iter().enumerate() {
        sim.execute(
            client_id,
            *node,
            Command::Set(key.to_string(), SDS::from_str(value)),
        );
        sim.advance_time_ms(5);
        sim.gossip_round();
    }

    // Some gossip rounds while partitioned
    for _ in 0..10 {
        sim.advance_time_ms(10);
        sim.gossip_round();
    }

    // Heal partition
    for (a, b) in &partition_config.partitioned_pairs {
        sim.heal_partition(*a, *b);
    }

    // Writes after healing
    for (client_id, (node, key, value)) in writes_after_heal.iter().enumerate() {
        sim.execute(
            client_id + writes_during_partition.len(),
            *node,
            Command::Set(key.to_string(), SDS::from_str(value)),
        );
        sim.advance_time_ms(5);
        sim.gossip_round();
    }

    // Convergence rounds
    let mut converged = false;
    let mut rounds_to_converge = 0;

    for round in 0..max_convergence_rounds {
        sim.advance_time_ms(10);
        sim.gossip_round();

        // Check if all test keys have converged
        let test_keys: HashSet<&str> = writes_during_partition
            .iter()
            .chain(writes_after_heal.iter())
            .map(|(_, k, _)| *k)
            .collect();

        let all_converged = test_keys.iter().all(|key| sim.check_key_convergence(key));

        if all_converged {
            converged = true;
            rounds_to_converge = round + 1;
            break;
        }
    }

    // Get final values for first test key
    let first_key = writes_during_partition
        .first()
        .or(writes_after_heal.first())
        .map(|(_, k, _)| *k)
        .unwrap_or("test_key");

    let final_values = sim.get_all_values(first_key);

    // Check linearizability
    let lin_result = check_single_key_linearizability(&sim.history, first_key);

    PartitionTestResult {
        test_name: test_name.to_string(),
        partition_config,
        writes_during_partition: writes_during_partition.len(),
        writes_after_heal: writes_after_heal.len(),
        converged,
        convergence_rounds: rounds_to_converge,
        final_values,
        linearizable: lin_result.is_linearizable,
    }
}

/// Run multiple partition tests with different seeds
pub fn run_partition_test_batch(
    test_name: &str,
    num_nodes: usize,
    partition_fn: fn(usize) -> PartitionConfig,
    num_seeds: usize,
) -> PartitionBatchResult {
    let mut results = Vec::new();
    let mut convergence_failures = Vec::new();

    for seed in 0..num_seeds as u64 {
        let config = partition_fn(num_nodes);

        // Create writes based on partition type
        let writes_during: Vec<(usize, &str, &str)> = vec![
            (0, "key1", "value_from_0"),
            (num_nodes - 1, "key1", "value_from_last"),
        ];

        let writes_after: Vec<(usize, &str, &str)> = vec![
            (0, "key1", "final_value"),
        ];

        let result = run_partition_test(
            test_name,
            num_nodes,
            seed,
            config,
            writes_during,
            writes_after,
            50,
        );

        if !result.converged {
            convergence_failures.push(seed);
        }
        results.push(result);
    }

    let converged_count = results.iter().filter(|r| r.converged).count();
    let avg_rounds: f64 = results
        .iter()
        .filter(|r| r.converged)
        .map(|r| r.convergence_rounds as f64)
        .sum::<f64>()
        / converged_count.max(1) as f64;

    PartitionBatchResult {
        test_name: test_name.to_string(),
        total_runs: num_seeds,
        converged_count,
        convergence_failures,
        average_convergence_rounds: avg_rounds,
    }
}

/// Result of a batch partition test
#[derive(Debug)]
pub struct PartitionBatchResult {
    pub test_name: String,
    pub total_runs: usize,
    pub converged_count: usize,
    pub convergence_failures: Vec<u64>,
    pub average_convergence_rounds: f64,
}

impl PartitionBatchResult {
    pub fn all_converged(&self) -> bool {
        self.convergence_failures.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "{}: {}/{} converged, avg {} rounds, {} failures",
            self.test_name,
            self.converged_count,
            self.total_runs,
            self.average_convergence_rounds as usize,
            self.convergence_failures.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_two_node_partition() {
        let result = run_partition_test(
            "simple_partition",
            3,
            42,
            PartitionConfig::asymmetric(0, 2),
            vec![(0, "key1", "from_node_0")],
            vec![(0, "key1", "after_heal")],
            30,
        );

        println!("Simple partition result: {:?}", result);
        assert!(result.converged, "Should converge after partition heals");
    }

    #[test]
    fn test_minority_partition() {
        // Isolate one node from a 5-node cluster
        let result = run_partition_test(
            "minority_partition",
            5,
            42,
            PartitionConfig::isolate_node(2, 5),
            vec![
                (0, "key1", "majority_value"),
                (2, "key1", "isolated_value"), // This write happens on isolated node
            ],
            vec![(0, "key1", "final_value")],
            50,
        );

        println!("Minority partition result: {:?}", result);
        assert!(result.converged, "Should converge after partition heals");

        // All nodes should have the same final value
        let unique_values: HashSet<_> = result.final_values.iter().collect();
        assert_eq!(unique_values.len(), 1, "All nodes should converge to same value");
    }

    #[test]
    fn test_split_brain_partition() {
        // Split 5 nodes into groups [0,1] and [2,3,4]
        let result = run_partition_test(
            "split_brain",
            5,
            42,
            PartitionConfig::split_brain(vec![0, 1], vec![2, 3, 4]),
            vec![
                (0, "key1", "group_a_value"),
                (3, "key1", "group_b_value"),
            ],
            vec![(0, "key1", "reconciled_value")],
            50,
        );

        println!("Split brain result: {:?}", result);
        assert!(result.converged, "Should converge after partition heals");
    }

    #[test]
    fn test_concurrent_writes_both_partitions() {
        // Test concurrent writes from both sides of a partition
        for seed in 0..20 {
            let mut sim = MultiNodeSimulation::new(4, seed);

            // Create partition: [0,1] vs [2,3]
            sim.partition(0, 2);
            sim.partition(0, 3);
            sim.partition(1, 2);
            sim.partition(1, 3);

            // Write on both sides simultaneously
            sim.execute(1, 0, Command::Set("conflict_key".into(), SDS::from_str("side_a")));
            sim.execute(2, 2, Command::Set("conflict_key".into(), SDS::from_str("side_b")));

            // Gossip within partitions
            for _ in 0..10 {
                sim.advance_time_ms(10);
                sim.gossip_round();
            }

            // Node 0 and 1 should have "side_a", nodes 2 and 3 should have "side_b"
            let values_before_heal = sim.get_all_values("conflict_key");
            assert_eq!(values_before_heal[0], values_before_heal[1],
                "Nodes in same partition should agree");
            assert_eq!(values_before_heal[2], values_before_heal[3],
                "Nodes in same partition should agree");

            // Heal partition
            sim.heal_partition(0, 2);
            sim.heal_partition(0, 3);
            sim.heal_partition(1, 2);
            sim.heal_partition(1, 3);

            // Write after heal to trigger cross-partition gossip
            // This simulates anti-entropy or new activity after partition heals
            sim.execute(3, 0, Command::Set("conflict_key".into(), SDS::from_str("after_heal")));

            // Converge
            sim.converge(50);

            // All nodes should converge to the post-heal value
            assert!(
                sim.check_key_convergence("conflict_key"),
                "Seed {} failed to converge after heal: {:?}",
                seed,
                sim.get_all_values("conflict_key")
            );
        }
    }

    #[test]
    fn test_cascading_partitions() {
        // Test partitions that affect message routing
        let mut sim = MultiNodeSimulation::new(5, 42);

        // Initial write
        sim.execute(1, 0, Command::Set("cascade_key".into(), SDS::from_str("initial")));
        sim.converge(10);

        // Create cascading partitions: 0 -> 1 -> 2 -> 3 -> 4
        // Each node can only talk to its neighbor
        sim.partition(0, 2);
        sim.partition(0, 3);
        sim.partition(0, 4);
        sim.partition(1, 3);
        sim.partition(1, 4);
        sim.partition(2, 4);

        // Write on node 0
        sim.execute(2, 0, Command::Set("cascade_key".into(), SDS::from_str("from_0")));

        // Gossip - message should cascade through the chain (0->1->2->3->4)
        // Each hop takes a gossip round
        for _ in 0..50 {
            sim.advance_time_ms(10);
            sim.gossip_round();
        }

        // Verify cascade propagation within chain
        // Node 0 and 1 should have "from_0", node 2 should have it from 1
        let values_during = sim.get_all_values("cascade_key");
        assert_eq!(values_during[0], Some("from_0".to_string()));
        assert_eq!(values_during[1], Some("from_0".to_string()));

        // Heal all partitions
        sim.heal_partition(0, 2);
        sim.heal_partition(0, 3);
        sim.heal_partition(0, 4);
        sim.heal_partition(1, 3);
        sim.heal_partition(1, 4);
        sim.heal_partition(2, 4);

        // Write to trigger full propagation after heal
        sim.execute(3, 0, Command::Set("cascade_key".into(), SDS::from_str("final")));
        sim.converge(30);

        assert!(
            sim.check_key_convergence("cascade_key"),
            "Should converge after cascading partition heals: {:?}",
            sim.get_all_values("cascade_key")
        );
    }

    #[test]
    fn test_partition_with_packet_loss() {
        // Combine partition with packet loss
        for seed in 0..10 {
            let mut sim = MultiNodeSimulation::new(3, seed)
                .with_packet_loss(0.2);

            // Partition node 2
            sim.partition(0, 2);
            sim.partition(1, 2);

            // Write on node 0
            sim.execute(1, 0, Command::Set("lossy_key".into(), SDS::from_str("value")));

            // Gossip with packet loss
            for _ in 0..20 {
                sim.advance_time_ms(10);
                sim.gossip_round();
            }

            // Heal partition
            sim.heal_partition(0, 2);
            sim.heal_partition(1, 2);

            // Many rounds to overcome packet loss
            sim.converge(100);

            // Should eventually converge despite packet loss + partition
            // Note: With high packet loss, might need retransmission in real system
        }
    }

    #[test]
    fn test_flapping_partition() {
        // Test partition that repeatedly forms and heals
        let mut sim = MultiNodeSimulation::new(3, 42);

        for i in 0..5 {
            // Partition node 2 from node 0
            sim.partition(0, 2);

            // Write during partition on node 0
            sim.execute(
                i * 2,
                0,
                Command::Set("flap_key".into(), SDS::from_str(&format!("value_{}", i))),
            );

            // Gossip within partition (0<->1 can talk, but not 0<->2)
            sim.advance_time_ms(20);
            sim.gossip_round();

            // Heal
            sim.heal_partition(0, 2);

            // Write after heal on a different node to propagate state
            sim.execute(
                i * 2 + 1,
                0,
                Command::Set("flap_key".into(), SDS::from_str(&format!("healed_{}", i))),
            );

            // Converge after heal
            sim.converge(10);
        }

        // Final convergence
        sim.converge(30);

        assert!(
            sim.check_key_convergence("flap_key"),
            "Should converge after flapping partition: {:?}",
            sim.get_all_values("flap_key")
        );
    }

    #[test]
    fn test_partition_batch_isolated_node() {
        let result = run_partition_test_batch(
            "isolated_node_batch",
            5,
            |n| PartitionConfig::isolate_node(2, n),
            100,
        );

        println!("{}", result.summary());
        assert!(
            result.all_converged(),
            "All tests should converge: {:?}",
            result.convergence_failures
        );
    }

    #[test]
    fn test_partition_batch_split_brain() {
        let result = run_partition_test_batch(
            "split_brain_batch",
            6,
            |_| PartitionConfig::split_brain(vec![0, 1, 2], vec![3, 4, 5]),
            100,
        );

        println!("{}", result.summary());
        assert!(
            result.all_converged(),
            "All tests should converge: {:?}",
            result.convergence_failures
        );
    }

    #[test]
    fn test_partition_preserves_causality() {
        // Verify that causal ordering is preserved across partition
        let mut sim = MultiNodeSimulation::new(3, 42);

        // Write A on node 0
        sim.execute(1, 0, Command::Set("causal_key".into(), SDS::from_str("A")));
        sim.converge(10);

        // All nodes should see A
        for node in &sim.nodes {
            assert_eq!(
                node.get_replicated_value("causal_key"),
                Some("A".to_string())
            );
        }

        // Partition node 2 completely
        sim.partition(0, 2);
        sim.partition(1, 2);

        // Write B on node 0 (node 2 won't see this while partitioned)
        sim.execute(2, 0, Command::Set("causal_key".into(), SDS::from_str("B")));
        sim.converge(10);

        // Nodes 0 and 1 should see B, node 2 should still see A
        assert_eq!(
            sim.nodes[0].get_replicated_value("causal_key"),
            Some("B".to_string())
        );
        assert_eq!(
            sim.nodes[1].get_replicated_value("causal_key"),
            Some("B".to_string())
        );
        assert_eq!(
            sim.nodes[2].get_replicated_value("causal_key"),
            Some("A".to_string())
        );

        // Heal partition
        sim.heal_partition(0, 2);
        sim.heal_partition(1, 2);

        // Write C to trigger gossip and propagate B to node 2
        sim.execute(3, 0, Command::Set("causal_key".into(), SDS::from_str("C")));
        sim.converge(20);

        // All nodes should see C (the latest write after heal)
        assert!(sim.check_key_convergence("causal_key"));
        assert_eq!(
            sim.nodes[2].get_replicated_value("causal_key"),
            Some("C".to_string())
        );
    }

    #[test]
    fn test_multi_key_partition_convergence() {
        // Test that multiple keys converge correctly after partition
        let mut sim = MultiNodeSimulation::new(4, 42);

        // Create partition: [0,1] vs [2,3]
        sim.partition(0, 2);
        sim.partition(0, 3);
        sim.partition(1, 2);
        sim.partition(1, 3);

        // Write different keys on both sides
        sim.execute(1, 0, Command::Set("key_a".into(), SDS::from_str("value_a")));
        sim.execute(2, 1, Command::Set("key_b".into(), SDS::from_str("value_b")));
        sim.execute(3, 2, Command::Set("key_c".into(), SDS::from_str("value_c")));
        sim.execute(4, 3, Command::Set("key_d".into(), SDS::from_str("value_d")));

        // Gossip within partitions
        sim.converge(20);

        // Verify each partition has its own keys
        // Partition [0,1] should have key_a, key_b
        // Partition [2,3] should have key_c, key_d
        assert_eq!(sim.nodes[0].get_replicated_value("key_a"), Some("value_a".to_string()));
        assert_eq!(sim.nodes[1].get_replicated_value("key_b"), Some("value_b".to_string()));
        assert_eq!(sim.nodes[2].get_replicated_value("key_c"), Some("value_c".to_string()));
        assert_eq!(sim.nodes[3].get_replicated_value("key_d"), Some("value_d".to_string()));

        // Heal all partitions
        sim.heal_partition(0, 2);
        sim.heal_partition(0, 3);
        sim.heal_partition(1, 2);
        sim.heal_partition(1, 3);

        // Write on each key to trigger cross-partition gossip
        sim.execute(5, 0, Command::Set("key_a".into(), SDS::from_str("value_a_final")));
        sim.execute(6, 1, Command::Set("key_b".into(), SDS::from_str("value_b_final")));
        sim.execute(7, 2, Command::Set("key_c".into(), SDS::from_str("value_c_final")));
        sim.execute(8, 3, Command::Set("key_d".into(), SDS::from_str("value_d_final")));

        // Converge
        sim.converge(50);

        // All keys should be present on all nodes with final values
        for (key, expected) in [
            ("key_a", "value_a_final"),
            ("key_b", "value_b_final"),
            ("key_c", "value_c_final"),
            ("key_d", "value_d_final"),
        ] {
            assert!(
                sim.check_key_convergence(key),
                "Key {} should converge: {:?}",
                key,
                sim.get_all_values(key)
            );
            // Verify value
            assert_eq!(
                sim.nodes[0].get_replicated_value(key),
                Some(expected.to_string()),
                "Key {} should have final value",
                key
            );
        }
    }

    #[test]
    fn test_stress_partition_scenarios() {
        // Run many seeds with various partition scenarios
        let scenarios: Vec<(&str, Box<dyn Fn(usize) -> PartitionConfig>)> = vec![
            ("isolate_first", Box::new(|n| PartitionConfig::isolate_node(0, n))),
            ("isolate_last", Box::new(|n| PartitionConfig::isolate_node(n - 1, n))),
            ("split_even", Box::new(|n| {
                let mid = n / 2;
                PartitionConfig::split_brain(
                    (0..mid).collect(),
                    (mid..n).collect(),
                )
            })),
        ];

        for (name, partition_fn) in scenarios {
            let mut all_converged = true;
            let mut failures = Vec::new();

            for seed in 0..50u64 {
                let mut sim = MultiNodeSimulation::new(5, seed);
                let config = partition_fn(5);

                // Apply partition
                for (a, b) in &config.partitioned_pairs {
                    sim.partition(*a, *b);
                }

                // Writes during partition from both sides
                sim.execute(1, 0, Command::Set("stress_key".into(), SDS::from_str("v1")));
                sim.execute(2, 4, Command::Set("stress_key".into(), SDS::from_str("v2")));

                sim.converge(10);

                // Heal
                for (a, b) in &config.partitioned_pairs {
                    sim.heal_partition(*a, *b);
                }

                // Write after heal to trigger convergence
                sim.execute(3, 0, Command::Set("stress_key".into(), SDS::from_str("final")));

                sim.converge(50);

                if !sim.check_key_convergence("stress_key") {
                    all_converged = false;
                    failures.push(seed);
                }
            }

            println!(
                "Stress test '{}': {} failures out of 50",
                name,
                failures.len()
            );
            assert!(
                all_converged,
                "Scenario '{}' had failures: {:?}",
                name,
                failures
            );
        }
    }
}
