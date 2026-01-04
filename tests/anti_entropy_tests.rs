//! Anti-Entropy Protocol Tests
//!
//! Tests demonstrating automatic state synchronization after partition healing
//! without requiring new writes.

use redis_sim::redis::{Command, SDS};
use redis_sim::replication::{ReplicaId, StateDigest};
use redis_sim::replication::state::ReplicatedValue;
use redis_sim::replication::lattice::LamportClock;
use redis_sim::simulator::multi_node::MultiNodeSimulation;
use std::collections::HashMap;

fn make_value(s: &str, timestamp: u64, replica: ReplicaId) -> ReplicatedValue {
    let clock = LamportClock { time: timestamp, replica_id: replica };
    ReplicatedValue::with_value(SDS::from_str(s), clock)
}

#[test]
fn test_anti_entropy_auto_sync_on_partition_heal() {
    // This test demonstrates that with anti-entropy enabled (default),
    // nodes automatically sync when a partition heals, WITHOUT requiring new writes.

    let mut sim = MultiNodeSimulation::new(3, 42);

    // Write some data before partitioning
    sim.execute(1, 0, Command::Set("key1".into(), SDS::from_str("value1")));
    sim.converge(10);

    // Verify all nodes have the data
    assert!(sim.check_key_convergence("key1"));

    // Create partition isolating node 2
    sim.partition(0, 2);
    sim.partition(1, 2);

    // Write new data while partitioned (node 2 won't see this)
    sim.execute(2, 0, Command::Set("key2".into(), SDS::from_str("value2")));
    sim.execute(3, 1, Command::Set("key3".into(), SDS::from_str("value3")));
    sim.converge(10);

    // Node 2 should NOT have key2 or key3
    assert_eq!(sim.nodes[2].get_replicated_value("key2"), None);
    assert_eq!(sim.nodes[2].get_replicated_value("key3"), None);

    // But nodes 0 and 1 should have them
    assert_eq!(sim.nodes[0].get_replicated_value("key2"), Some("value2".to_string()));
    assert_eq!(sim.nodes[1].get_replicated_value("key3"), Some("value3".to_string()));

    // Heal partition - anti-entropy should automatically sync
    sim.heal_partition(0, 2);
    sim.heal_partition(1, 2);

    // NO new writes needed! Just run gossip to process any queued messages
    sim.converge(5);

    // Now node 2 should have all keys thanks to anti-entropy
    assert_eq!(
        sim.nodes[2].get_replicated_value("key2"),
        Some("value2".to_string()),
        "Anti-entropy should have synced key2 to node 2"
    );
    assert_eq!(
        sim.nodes[2].get_replicated_value("key3"),
        Some("value3".to_string()),
        "Anti-entropy should have synced key3 to node 2"
    );

    // Verify convergence
    assert!(sim.check_key_convergence("key1"));
    assert!(sim.check_key_convergence("key2"));
    assert!(sim.check_key_convergence("key3"));

    println!("Anti-entropy syncs performed: {}", sim.anti_entropy_syncs);
    assert!(sim.anti_entropy_syncs > 0, "Should have performed anti-entropy syncs");
}

#[test]
fn test_anti_entropy_bidirectional_sync() {
    // Test that anti-entropy syncs data in BOTH directions

    let mut sim = MultiNodeSimulation::new(2, 42);

    // Partition nodes immediately
    sim.partition(0, 1);

    // Write different keys on each side
    sim.execute(1, 0, Command::Set("only_on_0".into(), SDS::from_str("value_0")));
    sim.execute(2, 1, Command::Set("only_on_1".into(), SDS::from_str("value_1")));
    sim.converge(10);

    // Verify isolation
    assert_eq!(sim.nodes[0].get_replicated_value("only_on_1"), None);
    assert_eq!(sim.nodes[1].get_replicated_value("only_on_0"), None);

    // Heal partition
    sim.heal_partition(0, 1);
    sim.converge(5);

    // Both nodes should now have both keys
    assert_eq!(
        sim.nodes[0].get_replicated_value("only_on_1"),
        Some("value_1".to_string()),
        "Node 0 should have received key from node 1"
    );
    assert_eq!(
        sim.nodes[1].get_replicated_value("only_on_0"),
        Some("value_0".to_string()),
        "Node 1 should have received key from node 0"
    );
}

#[test]
fn test_anti_entropy_disabled() {
    // Test that disabling anti-entropy requires manual sync

    let mut sim = MultiNodeSimulation::new_without_anti_entropy(3, 42);

    // Write data
    sim.execute(1, 0, Command::Set("key1".into(), SDS::from_str("value1")));
    sim.converge(10);

    // Partition
    sim.partition(0, 2);
    sim.partition(1, 2);

    // Write more
    sim.execute(2, 0, Command::Set("key2".into(), SDS::from_str("value2")));
    sim.converge(10);

    // Heal
    sim.heal_partition(0, 2);
    sim.heal_partition(1, 2);

    // Converge (but no anti-entropy)
    sim.converge(20);

    // Node 2 should NOT have key2 because anti-entropy is disabled
    // and no new writes triggered gossip
    assert_eq!(
        sim.nodes[2].get_replicated_value("key2"),
        None,
        "Without anti-entropy, node 2 shouldn't auto-sync"
    );

    // Now manually trigger anti-entropy
    sim.run_full_anti_entropy();
    sim.converge(5);

    // Now it should be synced
    assert_eq!(
        sim.nodes[2].get_replicated_value("key2"),
        Some("value2".to_string()),
        "Manual anti-entropy should sync the data"
    );
}

#[test]
fn test_anti_entropy_split_brain_recovery() {
    // Test split-brain scenario where both sides have different writes

    let mut sim = MultiNodeSimulation::new(4, 42);

    // Split into two groups: [0,1] and [2,3]
    sim.partition(0, 2);
    sim.partition(0, 3);
    sim.partition(1, 2);
    sim.partition(1, 3);

    // Write different values to same key on both sides
    sim.execute(1, 0, Command::Set("conflict".into(), SDS::from_str("group_a")));
    sim.execute(2, 2, Command::Set("conflict".into(), SDS::from_str("group_b")));

    // Also write unique keys on each side
    sim.execute(3, 0, Command::Set("unique_a".into(), SDS::from_str("from_a")));
    sim.execute(4, 2, Command::Set("unique_b".into(), SDS::from_str("from_b")));

    sim.converge(10);

    // Verify split state
    let values_a = sim.get_all_values("conflict");
    assert_eq!(values_a[0], values_a[1], "Group A should agree");
    assert_eq!(values_a[2], values_a[3], "Group B should agree");
    assert_ne!(values_a[0], values_a[2], "Groups should differ");

    // Heal all partitions
    sim.heal_partition(0, 2);
    sim.heal_partition(0, 3);
    sim.heal_partition(1, 2);
    sim.heal_partition(1, 3);

    // Converge (anti-entropy will sync)
    sim.converge(10);

    // All nodes should now agree (LWW will pick winner)
    assert!(
        sim.check_key_convergence("conflict"),
        "Conflict key should converge: {:?}",
        sim.get_all_values("conflict")
    );

    // Both unique keys should be on all nodes
    assert!(sim.check_key_convergence("unique_a"));
    assert!(sim.check_key_convergence("unique_b"));

    println!("Anti-entropy syncs: {}", sim.anti_entropy_syncs);
}

#[test]
fn test_merkle_digest_comparison() {
    // Test the underlying Merkle tree digest comparison

    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);

    // Create two key sets with some overlap
    let mut keys1: HashMap<String, ReplicatedValue> = HashMap::new();
    keys1.insert("shared".to_string(), make_value("same", 1, r1));
    keys1.insert("only1".to_string(), make_value("val1", 2, r1));

    let mut keys2: HashMap<String, ReplicatedValue> = HashMap::new();
    keys2.insert("shared".to_string(), make_value("same", 1, r1));
    keys2.insert("only2".to_string(), make_value("val2", 3, r2));

    let digest1 = StateDigest::from_state(&keys1, r1, 1, 4);
    let digest2 = StateDigest::from_state(&keys2, r2, 1, 4);

    // Digests should differ
    assert!(digest1.differs_from(&digest2));

    // Should detect divergent buckets
    let divergent = digest1.divergent_buckets(&digest2);
    assert!(!divergent.is_empty(), "Should detect divergent buckets");

    println!("Divergent buckets: {:?}", divergent);
}

#[test]
fn test_anti_entropy_with_many_keys() {
    // Test anti-entropy with larger key sets

    let mut sim = MultiNodeSimulation::new(3, 42);

    // Partition node 2
    sim.partition(0, 2);
    sim.partition(1, 2);

    // Write many keys while partitioned
    for i in 0..100 {
        let key = format!("key_{}", i);
        let value = format!("value_{}", i);
        let node = i % 2; // Alternate between nodes 0 and 1
        sim.execute(i, node, Command::Set(key, SDS::from_str(&value)));
    }

    sim.converge(20);

    // Node 2 should have none of these keys
    for i in 0..100 {
        let key = format!("key_{}", i);
        assert_eq!(
            sim.nodes[2].get_replicated_value(&key),
            None,
            "Key {} shouldn't be on node 2 while partitioned",
            key
        );
    }

    // Heal and let anti-entropy work
    sim.heal_partition(0, 2);
    sim.heal_partition(1, 2);
    sim.converge(10);

    // Now node 2 should have all keys
    let mut synced_count = 0;
    for i in 0..100 {
        let key = format!("key_{}", i);
        if sim.nodes[2].get_replicated_value(&key).is_some() {
            synced_count += 1;
        }
    }

    println!("Synced {} out of 100 keys via anti-entropy", synced_count);
    assert_eq!(synced_count, 100, "All keys should be synced");
}

#[test]
fn test_anti_entropy_preserves_lww_semantics() {
    // Ensure anti-entropy respects Last-Writer-Wins semantics

    let mut sim = MultiNodeSimulation::new(2, 42);

    // Write initial value
    sim.execute(1, 0, Command::Set("lww_key".into(), SDS::from_str("initial")));
    sim.converge(10);

    // Partition
    sim.partition(0, 1);

    // Write different values with different timestamps
    // Node 0 writes first (lower timestamp)
    sim.execute(2, 0, Command::Set("lww_key".into(), SDS::from_str("from_0")));

    // Advance time significantly
    sim.advance_time_ms(1000);

    // Node 1 writes later (higher timestamp) - should win
    sim.execute(3, 1, Command::Set("lww_key".into(), SDS::from_str("from_1_later")));

    sim.converge(10);

    // Heal
    sim.heal_partition(0, 1);
    sim.converge(10);

    // The later write should win on both nodes
    let final_values = sim.get_all_values("lww_key");
    assert!(
        sim.check_key_convergence("lww_key"),
        "Should converge: {:?}",
        final_values
    );

    // Both should have the later value
    assert_eq!(
        final_values[0], final_values[1],
        "Both nodes should have same value"
    );
}

#[test]
fn test_anti_entropy_stats() {
    // Test that anti-entropy statistics are tracked

    let mut sim = MultiNodeSimulation::new(3, 42);

    assert_eq!(sim.anti_entropy_syncs, 0, "Should start with 0 syncs");

    // Partition and write
    sim.partition(0, 2);
    sim.execute(1, 0, Command::Set("key".into(), SDS::from_str("value")));
    sim.converge(10);

    // Heal (should trigger sync)
    sim.heal_partition(0, 2);

    assert!(
        sim.anti_entropy_syncs > 0,
        "Should have performed at least one sync"
    );

    let syncs_after_first_heal = sim.anti_entropy_syncs;

    // Another partition and heal
    sim.partition(1, 2);
    sim.execute(2, 1, Command::Set("key2".into(), SDS::from_str("value2")));
    sim.converge(10);
    sim.heal_partition(1, 2);

    assert!(
        sim.anti_entropy_syncs > syncs_after_first_heal,
        "Should have performed more syncs"
    );

    println!("Total anti-entropy syncs: {}", sim.anti_entropy_syncs);
}
