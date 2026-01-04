//! Eventual consistency convergence tests
//!
//! Tests that verify multi-node CRDT-based replication converges correctly.
//! These tests validate the Anna-style eventual consistency guarantees.

use redis_sim::redis::SDS;
use redis_sim::replication::state::{ReplicationDelta, ShardReplicaState};
use redis_sim::replication::lattice::{GCounter, PNCounter, ReplicaId};
use redis_sim::replication::config::ConsistencyLevel;

/// Helper to convert SDS to string for comparison
fn sds_to_string(sds: &SDS) -> String {
    String::from_utf8_lossy(sds.as_bytes()).to_string()
}

/// Simulate multi-node eventual consistency convergence
#[test]
fn test_lww_convergence_concurrent_writes() {
    // Simulate 3 nodes writing concurrently to the same key
    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);
    let r3 = ReplicaId::new(3);

    let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
    let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);
    let mut state3 = ShardReplicaState::new(r3, ConsistencyLevel::Eventual);

    // Each node writes a different value to the same key "concurrently"
    // (In LWW, highest timestamp wins)
    let delta1 = state1.record_write("shared_key".to_string(), SDS::from_str("value_from_node1"), None);
    let delta2 = state2.record_write("shared_key".to_string(), SDS::from_str("value_from_node2"), None);
    let delta3 = state3.record_write("shared_key".to_string(), SDS::from_str("value_from_node3"), None);

    println!("=== LWW Convergence Test ===");
    println!("Delta1 timestamp: {:?}", delta1.value.timestamp);
    println!("Delta2 timestamp: {:?}", delta2.value.timestamp);
    println!("Delta3 timestamp: {:?}", delta3.value.timestamp);

    // Apply all deltas to all nodes (simulating gossip convergence)
    state1.apply_remote_delta(delta2.clone());
    state1.apply_remote_delta(delta3.clone());

    state2.apply_remote_delta(delta1.clone());
    state2.apply_remote_delta(delta3.clone());

    state3.apply_remote_delta(delta1.clone());
    state3.apply_remote_delta(delta2.clone());

    // All nodes should converge to the same value
    let val1 = state1.get_replicated("shared_key").unwrap().get().map(sds_to_string);
    let val2 = state2.get_replicated("shared_key").unwrap().get().map(sds_to_string);
    let val3 = state3.get_replicated("shared_key").unwrap().get().map(sds_to_string);

    println!("Node1 final value: {:?}", val1);
    println!("Node2 final value: {:?}", val2);
    println!("Node3 final value: {:?}", val3);

    assert_eq!(val1, val2, "Node1 and Node2 should converge");
    assert_eq!(val2, val3, "Node2 and Node3 should converge");
    println!("✓ All nodes converged to: {:?}", val1);
}

#[test]
fn test_lww_convergence_different_order() {
    // Test that convergence happens regardless of delta application order
    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);

    let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
    let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

    // Node1 writes first, then Node2 writes (higher timestamp wins)
    let delta1 = state1.record_write("key".to_string(), SDS::from_str("old_value"), None);
    let delta2 = state2.record_write("key".to_string(), SDS::from_str("new_value"), None);

    // Apply in different orders
    let mut state_a = ShardReplicaState::new(ReplicaId::new(10), ConsistencyLevel::Eventual);
    let mut state_b = ShardReplicaState::new(ReplicaId::new(11), ConsistencyLevel::Eventual);

    // State A: delta1 then delta2
    state_a.apply_remote_delta(delta1.clone());
    state_a.apply_remote_delta(delta2.clone());

    // State B: delta2 then delta1 (reverse order)
    state_b.apply_remote_delta(delta2.clone());
    state_b.apply_remote_delta(delta1.clone());

    let val_a = state_a.get_replicated("key").unwrap().get().map(sds_to_string);
    let val_b = state_b.get_replicated("key").unwrap().get().map(sds_to_string);

    println!("=== Order Independence Test ===");
    println!("State A (delta1→delta2): {:?}", val_a);
    println!("State B (delta2→delta1): {:?}", val_b);

    assert_eq!(val_a, val_b, "Order of delta application should not matter");
    println!("✓ Convergence is order-independent");
}

#[test]
fn test_delete_convergence() {
    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);

    let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
    let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

    // Node1 writes, Node2 deletes
    let write_delta = state1.record_write("key".to_string(), SDS::from_str("value"), None);

    // First apply write to state2 so it has the key
    state2.apply_remote_delta(write_delta.clone());

    // Then delete on state2
    let delete_delta = state2.record_delete("key".to_string()).unwrap();

    // Apply delete to state1
    state1.apply_remote_delta(delete_delta.clone());

    let val1 = state1.get_replicated("key");
    let val2 = state2.get_replicated("key");

    println!("=== Delete Convergence Test ===");
    println!("Node1 tombstone: {:?}", val1.map(|v| v.is_tombstone()));
    println!("Node2 tombstone: {:?}", val2.map(|v| v.is_tombstone()));

    // Both should see tombstone
    assert!(val1.unwrap().is_tombstone(), "Node1 should see tombstone");
    assert!(val2.unwrap().is_tombstone(), "Node2 should see tombstone");
    println!("✓ Delete converged correctly");
}

#[test]
fn test_gcounter_convergence() {
    // Test grow-only counter CRDT convergence
    let mut counter1 = GCounter::new();
    let mut counter2 = GCounter::new();
    let mut counter3 = GCounter::new();

    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);
    let r3 = ReplicaId::new(3);

    // Each node increments locally
    counter1.increment_by(r1, 5);
    counter2.increment_by(r2, 3);
    counter3.increment_by(r3, 7);

    println!("=== GCounter Convergence Test ===");
    println!("Counter1 local: {}", counter1.value());
    println!("Counter2 local: {}", counter2.value());
    println!("Counter3 local: {}", counter3.value());

    // Merge all counters
    let merged1 = counter1.merge(&counter2).merge(&counter3);
    let merged2 = counter2.merge(&counter1).merge(&counter3);
    let merged3 = counter3.merge(&counter1).merge(&counter2);

    println!("Merged1: {}", merged1.value());
    println!("Merged2: {}", merged2.value());
    println!("Merged3: {}", merged3.value());

    assert_eq!(merged1.value(), merged2.value());
    assert_eq!(merged2.value(), merged3.value());
    assert_eq!(merged1.value(), 15); // 5 + 3 + 7
    println!("✓ GCounter converged to: {}", merged1.value());
}

#[test]
fn test_pncounter_convergence() {
    // Test positive-negative counter CRDT convergence
    let mut counter1 = PNCounter::new();
    let mut counter2 = PNCounter::new();

    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);

    // Node1 increments, Node2 decrements
    counter1.increment_by(r1, 10);
    counter2.decrement_by(r2, 3);

    println!("=== PNCounter Convergence Test ===");
    println!("Counter1 local: {}", counter1.value());
    println!("Counter2 local: {}", counter2.value());

    // Merge
    let merged1 = counter1.merge(&counter2);
    let merged2 = counter2.merge(&counter1);

    println!("Merged1: {}", merged1.value());
    println!("Merged2: {}", merged2.value());

    assert_eq!(merged1.value(), merged2.value());
    assert_eq!(merged1.value(), 7); // 10 - 3
    println!("✓ PNCounter converged to: {}", merged1.value());
}

#[test]
fn test_causal_consistency_vector_clocks() {
    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);

    let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Causal);
    let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Causal);

    // Write on node1
    let delta1 = state1.record_write("key".to_string(), SDS::from_str("v1"), None);

    // Apply to node2, then write on node2 (causally after)
    state2.apply_remote_delta(delta1.clone());
    let delta2 = state2.record_write("key".to_string(), SDS::from_str("v2"), None);

    println!("=== Causal Consistency Test ===");
    println!("Delta1 VC: {:?}", delta1.value.vector_clock);
    println!("Delta2 VC: {:?}", delta2.value.vector_clock);

    // delta2 should be causally after delta1 (has vector clock)
    assert!(delta2.value.vector_clock.is_some(), "Causal mode should track vector clocks");

    // Apply delta2 to node1
    state1.apply_remote_delta(delta2.clone());

    let val1 = state1.get_replicated("key").unwrap().get().map(sds_to_string);
    let val2 = state2.get_replicated("key").unwrap().get().map(sds_to_string);

    println!("Node1 final: {:?}", val1);
    println!("Node2 final: {:?}", val2);

    assert_eq!(val1, val2);
    assert_eq!(val1, Some("v2".to_string())); // v2 is causally later
    println!("✓ Causal consistency preserved");
}

#[test]
fn test_partition_and_heal_convergence() {
    // Simulate network partition then healing
    let r1 = ReplicaId::new(1);
    let r2 = ReplicaId::new(2);

    let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
    let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

    println!("=== Partition & Heal Test ===");

    // Both nodes start with same value
    let initial = state1.record_write("key".to_string(), SDS::from_str("initial"), None);
    state2.apply_remote_delta(initial);

    // PARTITION: nodes write independently
    let delta1 = state1.record_write("key".to_string(), SDS::from_str("partition_value_1"), None);
    let delta2 = state2.record_write("key".to_string(), SDS::from_str("partition_value_2"), None);

    println!("During partition:");
    println!("  Node1: {:?}", state1.get_replicated("key").unwrap().get().map(sds_to_string));
    println!("  Node2: {:?}", state2.get_replicated("key").unwrap().get().map(sds_to_string));

    // HEAL: exchange deltas
    state1.apply_remote_delta(delta2.clone());
    state2.apply_remote_delta(delta1.clone());

    let val1 = state1.get_replicated("key").unwrap().get().map(sds_to_string);
    let val2 = state2.get_replicated("key").unwrap().get().map(sds_to_string);

    println!("After healing:");
    println!("  Node1: {:?}", val1);
    println!("  Node2: {:?}", val2);

    assert_eq!(val1, val2, "Nodes should converge after partition heals");
    println!("✓ Partition healed, nodes converged to: {:?}", val1);
}

#[test]
fn test_many_nodes_convergence() {
    // Test convergence with many nodes (simulating larger cluster)
    let num_nodes = 10;
    let mut states: Vec<ShardReplicaState> = (0..num_nodes)
        .map(|i| ShardReplicaState::new(ReplicaId::new(i as u64), ConsistencyLevel::Eventual))
        .collect();

    println!("=== Many Nodes Convergence Test ({} nodes) ===", num_nodes);

    // Each node writes to its own key
    let mut all_deltas: Vec<ReplicationDelta> = Vec::new();
    for (i, state) in states.iter_mut().enumerate() {
        let delta = state.record_write(
            format!("key_{}", i),
            SDS::from_str(&format!("value_from_node_{}", i)),
            None,
        );
        all_deltas.push(delta);
    }

    // Also have some nodes write to a shared key
    let shared_deltas: Vec<ReplicationDelta> = states.iter_mut()
        .take(5)
        .enumerate()
        .map(|(i, state)| {
            state.record_write(
                "shared".to_string(),
                SDS::from_str(&format!("shared_from_{}", i)),
                None,
            )
        })
        .collect();

    // Propagate all deltas to all nodes (full mesh gossip)
    for delta in &all_deltas {
        for state in &mut states {
            state.apply_remote_delta(delta.clone());
        }
    }
    for delta in &shared_deltas {
        for state in &mut states {
            state.apply_remote_delta(delta.clone());
        }
    }

    // Verify all nodes have all keys
    for (i, state) in states.iter().enumerate() {
        for j in 0..num_nodes {
            let key = format!("key_{}", j);
            let val = state.get_replicated(&key);
            assert!(val.is_some(), "Node {} missing key_{}", i, j);
        }
    }

    // Verify shared key converged
    let shared_values: Vec<_> = states.iter()
        .map(|s| s.get_replicated("shared").unwrap().get().map(sds_to_string))
        .collect();

    let first = &shared_values[0];
    for (i, val) in shared_values.iter().enumerate() {
        assert_eq!(val, first, "Node {} has different shared value", i);
    }

    println!("✓ All {} nodes converged", num_nodes);
    println!("  Shared key value: {:?}", first);
}

#[test]
fn test_high_contention_convergence() {
    // Many writes to same key from different nodes
    let num_nodes = 5;
    let writes_per_node = 20;

    let mut states: Vec<ShardReplicaState> = (0..num_nodes)
        .map(|i| ShardReplicaState::new(ReplicaId::new(i as u64), ConsistencyLevel::Eventual))
        .collect();

    println!("=== High Contention Test ({} nodes, {} writes each) ===", num_nodes, writes_per_node);

    let mut all_deltas: Vec<ReplicationDelta> = Vec::new();

    // Each node does multiple writes to the same key
    for round in 0..writes_per_node {
        for (node_id, state) in states.iter_mut().enumerate() {
            let delta = state.record_write(
                "hot_key".to_string(),
                SDS::from_str(&format!("n{}_r{}", node_id, round)),
                None,
            );
            all_deltas.push(delta);
        }
    }

    println!("Total deltas: {}", all_deltas.len());

    // Apply all deltas to all nodes
    for delta in &all_deltas {
        for state in &mut states {
            state.apply_remote_delta(delta.clone());
        }
    }

    // All nodes should converge
    let final_values: Vec<_> = states.iter()
        .map(|s| s.get_replicated("hot_key").unwrap().get().map(sds_to_string))
        .collect();

    let first = &final_values[0];
    for (i, val) in final_values.iter().enumerate() {
        assert_eq!(val, first, "Node {} has different value: {:?} vs {:?}", i, val, first);
    }

    println!("✓ Converged under high contention to: {:?}", first);
}
