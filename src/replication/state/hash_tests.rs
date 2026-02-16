//! Hash CRDT tests for replication state module

#[cfg(test)]
mod tests {
    use crate::redis::SDS;
    use crate::replication::config::ConsistencyLevel;
    use crate::replication::lattice::ReplicaId;
    use crate::replication::state::ShardReplicaState;

    #[test]
    fn test_hash_write_and_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Write different fields from different replicas
        let delta1 = state1.record_hash_write(
            "myhash".to_string(),
            vec![("field1".to_string(), SDS::from_str("value1"))],
        );
        let delta2 = state2.record_hash_write(
            "myhash".to_string(),
            vec![("field2".to_string(), SDS::from_str("value2"))],
        );

        // Apply cross-replica deltas
        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        // Both replicas should have both fields
        let hash1 = state1.get_replicated("myhash").unwrap().get_hash().unwrap();
        let hash2 = state2.get_replicated("myhash").unwrap().get_hash().unwrap();

        assert_eq!(hash1.len(), 2, "state1 should have 2 fields");
        assert_eq!(hash2.len(), 2, "state2 should have 2 fields");

        // Values should match
        assert_eq!(
            hash1
                .get("field1")
                .and_then(|lww| lww.get())
                .map(|s| s.as_bytes()),
            hash2
                .get("field1")
                .and_then(|lww| lww.get())
                .map(|s| s.as_bytes()),
            "field1 values should match"
        );
        assert_eq!(
            hash1
                .get("field2")
                .and_then(|lww| lww.get())
                .map(|s| s.as_bytes()),
            hash2
                .get("field2")
                .and_then(|lww| lww.get())
                .map(|s| s.as_bytes()),
            "field2 values should match"
        );
    }

    #[test]
    fn test_hash_field_conflict_resolution() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Write same field from both replicas (concurrent conflict)
        let delta1 = state1.record_hash_write(
            "myhash".to_string(),
            vec![("field".to_string(), SDS::from_str("value_from_r1"))],
        );
        let delta2 = state2.record_hash_write(
            "myhash".to_string(),
            vec![("field".to_string(), SDS::from_str("value_from_r2"))],
        );

        // Apply cross-replica deltas
        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        // Both replicas should converge to the same value (LWW semantics)
        let val1 = state1.get_replicated("myhash").unwrap().hash_get("field");
        let val2 = state2.get_replicated("myhash").unwrap().hash_get("field");
        assert_eq!(
            val1.map(|s| s.as_bytes()),
            val2.map(|s| s.as_bytes()),
            "Concurrent hash field writes should converge to same value"
        );
    }

    #[test]
    fn test_hash_field_deletion() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Create hash with multiple fields
        let delta1 = state1.record_hash_write(
            "myhash".to_string(),
            vec![
                ("field1".to_string(), SDS::from_str("value1")),
                ("field2".to_string(), SDS::from_str("value2")),
            ],
        );
        state2.apply_remote_delta(delta1.clone());

        // Delete field1 from state1
        let delete_delta =
            state1.record_hash_delete("myhash".to_string(), vec!["field1".to_string()]);

        // Apply deletion to state2
        if let Some(d) = delete_delta {
            state2.apply_remote_delta(d);
        }

        // field1 should be tombstoned, field2 should remain
        let hash1 = state1.get_replicated("myhash").unwrap().get_hash().unwrap();
        let hash2 = state2.get_replicated("myhash").unwrap().get_hash().unwrap();

        // field1 should be tombstoned (value None or tombstone=true)
        let field1_lww_1 = hash1.get("field1").unwrap();
        let field1_lww_2 = hash2.get("field1").unwrap();
        assert!(
            field1_lww_1.tombstone,
            "field1 should be tombstoned in state1"
        );
        assert!(
            field1_lww_2.tombstone,
            "field1 should be tombstoned in state2"
        );

        // field2 should still have value
        assert_eq!(
            hash1
                .get("field2")
                .and_then(|lww| lww.get())
                .map(|s| s.as_bytes()),
            Some(b"value2".as_slice()),
            "field2 should still have value in state1"
        );
        assert_eq!(
            hash2
                .get("field2")
                .and_then(|lww| lww.get())
                .map(|s| s.as_bytes()),
            Some(b"value2".as_slice()),
            "field2 should still have value in state2"
        );
    }

    #[test]
    fn test_hash_delete_wins_over_concurrent_write() {
        // Test that delete-after-write wins in LWW semantics
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // r1 writes a field
        let write_delta = state1.record_hash_write(
            "myhash".to_string(),
            vec![("field".to_string(), SDS::from_str("value"))],
        );
        state2.apply_remote_delta(write_delta.clone());

        // r2 deletes the same field (happens after write due to clock)
        let delete_delta =
            state2.record_hash_delete("myhash".to_string(), vec!["field".to_string()]);

        // Apply delete to r1
        if let Some(d) = delete_delta {
            state1.apply_remote_delta(d);
        }

        // Both should have tombstoned field
        let hash1 = state1.get_replicated("myhash").unwrap().get_hash().unwrap();
        let hash2 = state2.get_replicated("myhash").unwrap().get_hash().unwrap();

        assert!(
            hash1.get("field").unwrap().tombstone,
            "field should be tombstoned in state1"
        );
        assert!(
            hash2.get("field").unwrap().tombstone,
            "field should be tombstoned in state2"
        );
    }

    /// DST-style multi-seed test for hash convergence
    #[test]
    fn test_hash_convergence_multi_seed() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        for seed in 0..50 {
            let mut hasher = DefaultHasher::new();
            seed.hash(&mut hasher);
            let hash_seed = hasher.finish();

            // Create 3 replicas
            let r1 = ReplicaId::new(1);
            let r2 = ReplicaId::new(2);
            let r3 = ReplicaId::new(3);

            let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
            let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);
            let mut state3 = ShardReplicaState::new(r3, ConsistencyLevel::Eventual);

            // Generate operations based on seed
            let num_ops = 10 + (hash_seed % 20) as usize;
            let mut deltas = Vec::new();

            for i in 0..num_ops {
                let op_type = (hash_seed.wrapping_add(i as u64)) % 3;
                let field = format!("field{}", (hash_seed.wrapping_add(i as u64)) % 5);
                let value = format!("value_{}_seed{}", i, seed);

                let delta = match op_type {
                    0 => state1.record_hash_write(
                        "hash".to_string(),
                        vec![(field, SDS::from_str(&value))],
                    ),
                    1 => state2.record_hash_write(
                        "hash".to_string(),
                        vec![(field, SDS::from_str(&value))],
                    ),
                    _ => state3.record_hash_write(
                        "hash".to_string(),
                        vec![(field, SDS::from_str(&value))],
                    ),
                };
                deltas.push(delta);
            }

            // Apply all deltas to all replicas
            for delta in &deltas {
                state1.apply_remote_delta(delta.clone());
                state2.apply_remote_delta(delta.clone());
                state3.apply_remote_delta(delta.clone());
            }

            // All replicas should converge
            let hash1 = state1.get_replicated("hash").map(|v| v.get_hash());
            let hash2 = state2.get_replicated("hash").map(|v| v.get_hash());
            let hash3 = state3.get_replicated("hash").map(|v| v.get_hash());

            // Verify all replicas have same field values
            if let (Some(Some(h1)), Some(Some(h2)), Some(Some(h3))) = (hash1, hash2, hash3) {
                assert_eq!(
                    h1.len(),
                    h2.len(),
                    "seed {}: hash1 and hash2 should have same field count",
                    seed
                );
                assert_eq!(
                    h2.len(),
                    h3.len(),
                    "seed {}: hash2 and hash3 should have same field count",
                    seed
                );

                for (field, lww1) in h1.iter() {
                    let lww2 = h2.get(field).unwrap_or_else(|| {
                        panic!("seed {}: field {} missing in hash2", seed, field)
                    });
                    let lww3 = h3.get(field).unwrap_or_else(|| {
                        panic!("seed {}: field {} missing in hash3", seed, field)
                    });

                    assert_eq!(
                        lww1.get().map(|s| s.as_bytes()),
                        lww2.get().map(|s| s.as_bytes()),
                        "seed {}: field {} value mismatch between hash1 and hash2",
                        seed,
                        field
                    );
                    assert_eq!(
                        lww2.get().map(|s| s.as_bytes()),
                        lww3.get().map(|s| s.as_bytes()),
                        "seed {}: field {} value mismatch between hash2 and hash3",
                        seed,
                        field
                    );
                }
            }
        }
    }

    /// Test hash invariants are maintained
    #[test]
    fn test_hash_invariants() {
        let r1 = ReplicaId::new(1);
        let mut state = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);

        // Create a hash and verify invariants
        state.record_hash_write(
            "test_hash".to_string(),
            vec![
                ("a".to_string(), SDS::from_str("1")),
                ("b".to_string(), SDS::from_str("2")),
                ("c".to_string(), SDS::from_str("3")),
            ],
        );

        let replicated = state.get_replicated("test_hash").unwrap();
        assert!(replicated.is_hash(), "Value should be hash type");

        // Verify the hash-specific invariants
        #[cfg(debug_assertions)]
        replicated.verify_hash_invariants();

        // Delete a field and verify invariants still hold
        state.record_hash_delete("test_hash".to_string(), vec!["b".to_string()]);

        let replicated = state.get_replicated("test_hash").unwrap();
        #[cfg(debug_assertions)]
        replicated.verify_hash_invariants();
    }
}
