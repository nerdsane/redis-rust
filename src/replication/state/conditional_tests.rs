//! SET NX/XX convergence tests for replication state module
//!
//! SET NX/XX are conditional writes evaluated locally at command execution.
//! The replication layer only sees the final value (if written).
//! These tests verify that conditional writes converge correctly.

#[cfg(test)]
mod tests {
    use crate::redis::SDS;
    use crate::replication::config::ConsistencyLevel;
    use crate::replication::lattice::ReplicaId;
    use crate::replication::state::ShardReplicaState;

    #[test]
    fn test_set_nx_concurrent_race() {
        // Simulate: Two replicas both try SET NX on same key
        // Both think key doesn't exist, both write - LWW resolves conflict
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Both replicas do SET NX (both think key doesn't exist)
        let delta1 = state1.record_write("lock".to_string(), SDS::from_str("owner_r1"), None);
        let delta2 = state2.record_write("lock".to_string(), SDS::from_str("owner_r2"), None);

        // Apply cross-replica deltas
        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        // Both should converge to same winner
        let val1 = state1.get_replicated("lock").unwrap().get();
        let val2 = state2.get_replicated("lock").unwrap().get();

        assert_eq!(
            val1.map(|s| String::from_utf8_lossy(s.as_bytes()).to_string()),
            val2.map(|s| String::from_utf8_lossy(s.as_bytes()).to_string()),
            "Concurrent SET NX should converge to same value via LWW"
        );
    }

    #[test]
    fn test_set_xx_after_initial_write() {
        // SET XX only succeeds if key exists
        // Test that XX updates converge correctly
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // r1 creates the key
        let delta1 = state1.record_write("config".to_string(), SDS::from_str("v1"), None);
        state2.apply_remote_delta(delta1);

        // Both replicas do SET XX (both see key exists)
        let delta2 = state1.record_write("config".to_string(), SDS::from_str("v2_from_r1"), None);
        let delta3 = state2.record_write("config".to_string(), SDS::from_str("v2_from_r2"), None);

        // Apply cross-replica deltas
        state1.apply_remote_delta(delta3.clone());
        state2.apply_remote_delta(delta2.clone());

        // Both should converge
        let val1 = state1.get_replicated("config").unwrap().get();
        let val2 = state2.get_replicated("config").unwrap().get();

        assert_eq!(
            val1.map(|s| String::from_utf8_lossy(s.as_bytes()).to_string()),
            val2.map(|s| String::from_utf8_lossy(s.as_bytes()).to_string()),
            "Concurrent SET XX should converge to same value via LWW"
        );
    }

    #[test]
    fn test_conditional_write_convergence_multi_seed() {
        // DST-style test: concurrent conditional writes always converge
        for seed in 0..20 {
            let r1 = ReplicaId::new(1);
            let r2 = ReplicaId::new(2);

            let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
            let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

            // Concurrent writes with seed-based values
            let delta1 = state1.record_write(
                format!("key_{}", seed),
                SDS::from_str(&format!("value_r1_{}", seed)),
                None,
            );
            let delta2 = state2.record_write(
                format!("key_{}", seed),
                SDS::from_str(&format!("value_r2_{}", seed)),
                None,
            );

            state1.apply_remote_delta(delta2.clone());
            state2.apply_remote_delta(delta1.clone());

            let val1 = state1
                .get_replicated(&format!("key_{}", seed))
                .unwrap()
                .get();
            let val2 = state2
                .get_replicated(&format!("key_{}", seed))
                .unwrap()
                .get();

            assert_eq!(
                val1.map(|s| String::from_utf8_lossy(s.as_bytes()).to_string()),
                val2.map(|s| String::from_utf8_lossy(s.as_bytes()).to_string()),
                "seed {}: conditional writes should converge",
                seed
            );
        }
    }
}
