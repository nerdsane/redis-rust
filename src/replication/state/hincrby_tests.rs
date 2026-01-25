//! HINCRBY convergence tests for replication state module
//!
//! IMPORTANT: HINCRBY uses LWW semantics, NOT a commutative CRDT counter.
//! This means concurrent increments from different replicas will NOT sum -
//! instead, the last-writer wins. This is a known limitation.

#[cfg(test)]
mod tests {
    use crate::redis::SDS;
    use crate::replication::config::ConsistencyLevel;
    use crate::replication::lattice::ReplicaId;
    use crate::replication::state::ShardReplicaState;

    #[test]
    fn test_hincrby_convergence_sequential() {
        // Test that sequential HINCRBY operations converge correctly
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // r1 sets initial value "10"
        let delta1 = state1.record_hash_write(
            "counters".to_string(),
            vec![("hits".to_string(), SDS::from_str("10"))],
        );
        state2.apply_remote_delta(delta1);

        // r1 increments to "15" (simulating HINCRBY 5)
        let delta2 = state1.record_hash_write(
            "counters".to_string(),
            vec![("hits".to_string(), SDS::from_str("15"))],
        );
        state2.apply_remote_delta(delta2);

        // Both should see "15"
        let val1 = state1.get_replicated("counters").unwrap().hash_get("hits");
        let val2 = state2.get_replicated("counters").unwrap().hash_get("hits");

        assert_eq!(val1.map(|s| s.to_string()), Some("15".to_string()));
        assert_eq!(val2.map(|s| s.to_string()), Some("15".to_string()));
    }

    #[test]
    fn test_hincrby_concurrent_lww_semantics() {
        // Test that concurrent HINCRBY operations converge via LWW
        // NOTE: This demonstrates that increments DON'T sum - LWW picks a winner
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        // Both start with same value "10"
        let init_delta = state1.record_hash_write(
            "counters".to_string(),
            vec![("hits".to_string(), SDS::from_str("10"))],
        );
        state2.apply_remote_delta(init_delta);

        // r1 increments to "15" (HINCRBY 5)
        let delta1 = state1.record_hash_write(
            "counters".to_string(),
            vec![("hits".to_string(), SDS::from_str("15"))],
        );

        // r2 concurrently increments to "13" (HINCRBY 3 from its view of "10")
        let delta2 = state2.record_hash_write(
            "counters".to_string(),
            vec![("hits".to_string(), SDS::from_str("13"))],
        );

        // Apply cross-replica deltas
        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        // Both should converge to SAME value (LWW winner)
        // NOT "18" (10+5+3) - increments don't commute in LWW
        let val1 = state1.get_replicated("counters").unwrap().hash_get("hits");
        let val2 = state2.get_replicated("counters").unwrap().hash_get("hits");

        assert_eq!(
            val1.map(|s| s.to_string()),
            val2.map(|s| s.to_string()),
            "Concurrent HINCRBY should converge to same value via LWW"
        );

        // The winner should be either "15" or "13" (not "18")
        let winner = val1.map(|s| s.to_string()).unwrap();
        assert!(
            winner == "15" || winner == "13",
            "LWW should pick one of the concurrent values, got: {}",
            winner
        );
    }

    #[test]
    fn test_hincrby_convergence_multi_seed() {
        // DST-style test: verify HINCRBY always converges across many seeds
        for seed in 0..20 {
            let r1 = ReplicaId::new(1);
            let r2 = ReplicaId::new(2);
            let r3 = ReplicaId::new(3);

            let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
            let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);
            let mut state3 = ShardReplicaState::new(r3, ConsistencyLevel::Eventual);

            // Simulate concurrent HINCRBY from all replicas
            let values = [
                (seed * 10) as i64,
                (seed * 10 + 5) as i64,
                (seed * 10 + 3) as i64,
            ];

            let delta1 = state1.record_hash_write(
                "counter".to_string(),
                vec![("val".to_string(), SDS::from_str(&values[0].to_string()))],
            );
            let delta2 = state2.record_hash_write(
                "counter".to_string(),
                vec![("val".to_string(), SDS::from_str(&values[1].to_string()))],
            );
            let delta3 = state3.record_hash_write(
                "counter".to_string(),
                vec![("val".to_string(), SDS::from_str(&values[2].to_string()))],
            );

            // All-to-all delta application
            state1.apply_remote_delta(delta2.clone());
            state1.apply_remote_delta(delta3.clone());
            state2.apply_remote_delta(delta1.clone());
            state2.apply_remote_delta(delta3.clone());
            state3.apply_remote_delta(delta1.clone());
            state3.apply_remote_delta(delta2.clone());

            // All three should converge
            let v1 = state1.get_replicated("counter").unwrap().hash_get("val");
            let v2 = state2.get_replicated("counter").unwrap().hash_get("val");
            let v3 = state3.get_replicated("counter").unwrap().hash_get("val");

            assert_eq!(
                v1.map(|s| s.to_string()),
                v2.map(|s| s.to_string()),
                "seed {}: states 1 and 2 should converge",
                seed
            );
            assert_eq!(
                v2.map(|s| s.to_string()),
                v3.map(|s| s.to_string()),
                "seed {}: states 2 and 3 should converge",
                seed
            );
        }
    }
}
