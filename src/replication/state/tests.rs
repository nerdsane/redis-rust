//! Basic tests for replication state module

#[cfg(test)]
mod tests {
    use crate::redis::SDS;
    use crate::replication::config::ConsistencyLevel;
    use crate::replication::lattice::ReplicaId;
    use crate::replication::state::ShardReplicaState;

    #[test]
    fn test_shard_replica_write_and_merge() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut state1 = ShardReplicaState::new(r1, ConsistencyLevel::Eventual);
        let mut state2 = ShardReplicaState::new(r2, ConsistencyLevel::Eventual);

        let delta1 = state1.record_write("key1".to_string(), SDS::from_str("value1"), None);
        let delta2 = state2.record_write("key1".to_string(), SDS::from_str("value2"), None);

        state1.apply_remote_delta(delta2.clone());
        state2.apply_remote_delta(delta1.clone());

        let val1 = state1.get_replicated("key1").unwrap().get();
        let val2 = state2.get_replicated("key1").unwrap().get();
        assert_eq!(val1, val2);
    }
}
