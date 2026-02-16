//! ReplicatedShardActor - Actor-based replicated shard operations
//!
//! This actor owns a ReplicatedShard exclusively, eliminating Arc<RwLock<>>.
//!
//! ## Design (TigerBeetle/FoundationDB inspired)
//!
//! ```text
//! ┌─────────────────────────┐        ┌─────────────────────────┐
//! │ ReplicatedShardedState  │──msg──▶│  ReplicatedShardActor   │
//! │  (coordinator)          │        │  (owns shard state)     │
//! └─────────────────────────┘        └─────────────────────────┘
//! ```

use crate::redis::{Command, CommandExecutor, RespValue};
use crate::replication::state::ShardReplicaState;
use crate::replication::{ConsistencyLevel, ReplicaId, ReplicationDelta};
use crate::simulator::VirtualTime;
use tokio::sync::{mpsc, oneshot};

/// Messages for controlling the ReplicatedShardActor
#[derive(Debug)]
pub enum ReplicatedShardMessage {
    /// Execute a command and return result with optional delta
    Execute {
        cmd: Command,
        response: oneshot::Sender<(RespValue, Option<ReplicationDelta>)>,
    },
    /// Execute a read-only command (no delta generation)
    ExecuteReadonly {
        cmd: Command,
        response: oneshot::Sender<RespValue>,
    },
    /// Apply a remote delta from another replica
    ApplyRemoteDelta { delta: ReplicationDelta },
    /// Drain all pending deltas for replication
    DrainPendingDeltas {
        response: oneshot::Sender<Vec<ReplicationDelta>>,
    },
    /// Evict expired keys
    EvictExpired {
        current_time: VirtualTime,
        response: oneshot::Sender<usize>,
    },
    /// Get snapshot of replicated keys for checkpointing
    GetSnapshot {
        response: oneshot::Sender<
            std::collections::HashMap<String, crate::replication::state::ReplicatedValue>,
        >,
    },
    /// Apply recovered state from persistence
    ApplyRecoveredState {
        key: String,
        value: crate::replication::state::ReplicatedValue,
    },
    /// Graceful shutdown
    Shutdown { response: oneshot::Sender<()> },
}

/// Handle for communicating with the ReplicatedShardActor
#[derive(Clone)]
pub struct ReplicatedShardHandle {
    tx: mpsc::UnboundedSender<ReplicatedShardMessage>,
    shard_id: usize,
}

impl ReplicatedShardHandle {
    /// Execute a command and return result with optional delta
    #[inline]
    pub async fn execute(&self, cmd: Command) -> (RespValue, Option<ReplicationDelta>) {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(ReplicatedShardMessage::Execute { cmd, response: tx })
            .is_err()
        {
            return (RespValue::err("ERR shard unavailable"), None);
        }
        rx.await
            .unwrap_or_else(|_| (RespValue::err("ERR shard response failed"), None))
    }

    /// Execute a read-only command
    #[inline]
    pub async fn execute_readonly(&self, cmd: Command) -> RespValue {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(ReplicatedShardMessage::ExecuteReadonly { cmd, response: tx })
            .is_err()
        {
            return RespValue::err("ERR shard unavailable");
        }
        rx.await
            .unwrap_or_else(|_| RespValue::err("ERR shard response failed"))
    }

    /// Apply a remote delta (fire-and-forget)
    #[inline]
    pub fn apply_remote_delta(&self, delta: ReplicationDelta) {
        let _ = self
            .tx
            .send(ReplicatedShardMessage::ApplyRemoteDelta { delta });
    }

    /// Drain pending deltas for replication
    pub async fn drain_pending_deltas(&self) -> Vec<ReplicationDelta> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(ReplicatedShardMessage::DrainPendingDeltas { response: tx })
            .is_err()
        {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Evict expired keys
    pub async fn evict_expired(&self, current_time: VirtualTime) -> usize {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(ReplicatedShardMessage::EvictExpired {
                current_time,
                response: tx,
            })
            .is_err()
        {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    /// Get snapshot for checkpointing
    pub async fn get_snapshot(
        &self,
    ) -> std::collections::HashMap<String, crate::replication::state::ReplicatedValue> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(ReplicatedShardMessage::GetSnapshot { response: tx })
            .is_err()
        {
            return std::collections::HashMap::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Apply recovered state (fire-and-forget)
    pub fn apply_recovered_state(
        &self,
        key: String,
        value: crate::replication::state::ReplicatedValue,
    ) {
        let _ = self
            .tx
            .send(ReplicatedShardMessage::ApplyRecoveredState { key, value });
    }

    /// Graceful shutdown
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(ReplicatedShardMessage::Shutdown { response: tx })
            .is_ok()
        {
            let _ = rx.await;
        }
    }

    /// Get the shard ID
    pub fn shard_id(&self) -> usize {
        self.shard_id
    }

    /// Check if the actor is still running
    pub fn is_running(&self) -> bool {
        !self.tx.is_closed()
    }
}

/// The ReplicatedShardActor owns its state exclusively - no Arc<RwLock<>> needed!
pub struct ReplicatedShardActor {
    executor: CommandExecutor,
    replica_state: ShardReplicaState,
    rx: mpsc::UnboundedReceiver<ReplicatedShardMessage>,
    shard_id: usize,
}

impl ReplicatedShardActor {
    /// Spawn a new ReplicatedShardActor and return the handle
    pub fn spawn(
        replica_id: ReplicaId,
        consistency_level: ConsistencyLevel,
        shard_id: usize,
    ) -> ReplicatedShardHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let actor = ReplicatedShardActor {
            executor: CommandExecutor::new(),
            replica_state: ShardReplicaState::new(replica_id, consistency_level),
            rx,
            shard_id,
        };

        tokio::spawn(actor.run());

        ReplicatedShardHandle { tx, shard_id }
    }

    /// Run the actor's message loop
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                ReplicatedShardMessage::Execute { cmd, response } => {
                    let result = self.executor.execute(&cmd);
                    let delta = self.record_mutation_post_execute(&cmd);
                    let _ = response.send((result, delta));

                    #[cfg(debug_assertions)]
                    self.verify_invariants();
                }

                ReplicatedShardMessage::ExecuteReadonly { cmd, response } => {
                    let result = self.executor.execute_readonly(&cmd);
                    let _ = response.send(result);
                }

                ReplicatedShardMessage::ApplyRemoteDelta { delta } => {
                    self.apply_remote_delta_impl(delta);

                    #[cfg(debug_assertions)]
                    self.verify_invariants();
                }

                ReplicatedShardMessage::DrainPendingDeltas { response } => {
                    let deltas = self.replica_state.drain_pending_deltas();
                    let _ = response.send(deltas);
                }

                ReplicatedShardMessage::EvictExpired {
                    current_time,
                    response,
                } => {
                    let evicted = self.executor.evict_expired_direct(current_time);
                    let _ = response.send(evicted);
                }

                ReplicatedShardMessage::GetSnapshot { response } => {
                    let snapshot = self.replica_state.replicated_keys.clone();
                    let _ = response.send(snapshot);
                }

                ReplicatedShardMessage::ApplyRecoveredState { key, value } => {
                    // Insert into replica state
                    self.replica_state
                        .replicated_keys
                        .insert(key.clone(), value.clone());

                    // Also apply to executor for command execution
                    if value.is_hash() {
                        // Recover hash data
                        if let Some(hash) = value.get_hash() {
                            let pairs: Vec<(crate::redis::SDS, crate::redis::SDS)> = hash
                                .iter()
                                .filter_map(|(field, lww)| {
                                    lww.get()
                                        .map(|v| (crate::redis::SDS::from_str(field), v.clone()))
                                })
                                .collect();
                            if !pairs.is_empty() {
                                let cmd = Command::HSet(key, pairs);
                                self.executor.execute(&cmd);
                            }
                        }
                    } else if let Some(v) = value.get() {
                        if let Some(expiry_ms) = value.expiry_ms {
                            let seconds = (expiry_ms / 1000) as i64;
                            let cmd = Command::setex(key, seconds, v.clone());
                            self.executor.execute(&cmd);
                        } else {
                            let cmd = Command::set(key, v.clone());
                            self.executor.execute(&cmd);
                        }
                    }
                }

                ReplicatedShardMessage::Shutdown { response } => {
                    let _ = response.send(());
                    break;
                }
            }
        }
    }

    /// Record mutation after command execution
    fn record_mutation_post_execute(&mut self, cmd: &Command) -> Option<ReplicationDelta> {
        match cmd {
            Command::Set {
                key,
                value,
                ex,
                px,
                nx,
                xx,
                ..
            } => {
                // Calculate expiry in milliseconds
                let expiry_ms = match (ex, px) {
                    (Some(seconds), _) => Some(*seconds as u64 * 1000),
                    (_, Some(millis)) => Some(*millis as u64),
                    _ => None,
                };

                // For NX: only record if key existed (command was no-op)
                if *nx {
                    if let Some(v) = self.executor.get_data().get(key) {
                        if v.as_string().is_some() {
                            return Some(self.replica_state.record_write(
                                key.clone(),
                                value.clone(),
                                expiry_ms,
                            ));
                        }
                    }
                    return None;
                }

                // For XX: only record if key exists
                if *xx {
                    if self.executor.get_data().get(key).is_none() {
                        return None;
                    }
                }

                Some(
                    self.replica_state
                        .record_write(key.clone(), value.clone(), expiry_ms),
                )
            }
            Command::Del(keys) => {
                // Record deletion for each key
                let mut result = None;
                for key in keys {
                    result = self.replica_state.record_delete(key.clone());
                }
                result
            }
            Command::Incr(key)
            | Command::Decr(key)
            | Command::IncrBy(key, _)
            | Command::DecrBy(key, _)
            | Command::Append(key, _)
            | Command::GetSet(key, _) => {
                if let Some(value) = self.executor.get_data().get(key) {
                    if let Some(sds) = value.as_string() {
                        return Some(self.replica_state.record_write(
                            key.clone(),
                            sds.clone(),
                            None,
                        ));
                    }
                }
                None
            }
            // Hash commands
            Command::HSet(key, pairs) => {
                // TigerStyle: Preconditions
                debug_assert!(!key.is_empty(), "Precondition: HSet key must not be empty");
                debug_assert!(
                    !pairs.is_empty(),
                    "Precondition: HSet pairs must not be empty"
                );

                let fields: Vec<(String, crate::redis::SDS)> = pairs
                    .iter()
                    .map(|(f, v)| (f.to_string(), v.clone()))
                    .collect();
                let delta = self.replica_state.record_hash_write(key.clone(), fields);

                // TigerStyle: Postconditions
                #[cfg(debug_assertions)]
                {
                    debug_assert!(
                        self.replica_state.replicated_keys.contains_key(key),
                        "Postcondition: key must exist in replicated_keys after HSet"
                    );
                    debug_assert!(
                        self.replica_state
                            .replicated_keys
                            .get(key)
                            .map(|v| v.is_hash())
                            .unwrap_or(false),
                        "Postcondition: value must be hash type after HSet"
                    );
                }

                Some(delta)
            }
            Command::HDel(key, fields) => {
                // TigerStyle: Preconditions
                debug_assert!(!key.is_empty(), "Precondition: HDel key must not be empty");
                debug_assert!(
                    !fields.is_empty(),
                    "Precondition: HDel fields must not be empty"
                );

                let field_names: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
                self.replica_state
                    .record_hash_delete(key.clone(), field_names)
            }
            Command::HIncrBy(key, field, _) => {
                // TigerStyle: Preconditions
                debug_assert!(
                    !key.is_empty(),
                    "Precondition: HIncrBy key must not be empty"
                );
                debug_assert!(
                    !field.is_empty(),
                    "Precondition: HIncrBy field must not be empty"
                );

                // After HIncrBy, read the resulting hash value and record it
                if let Some(value) = self.executor.get_data().get(key) {
                    if let Some(hash) = value.as_hash() {
                        if let Some(field_value) =
                            hash.get(&crate::redis::SDS::from_str(&field.to_string()))
                        {
                            return Some(self.replica_state.record_hash_write(
                                key.clone(),
                                vec![(field.to_string(), field_value.clone())],
                            ));
                        }
                    }
                }
                None
            }
            Command::FlushDb | Command::FlushAll => None,
            _ => None,
        }
    }

    /// Apply a remote delta from another replica
    /// TigerStyle: Applies remote delta and syncs executor state
    fn apply_remote_delta_impl(&mut self, delta: ReplicationDelta) {
        // TigerStyle: Preconditions
        debug_assert!(
            !delta.key.is_empty(),
            "Precondition: delta key must not be empty"
        );

        self.replica_state.apply_remote_delta(delta.clone());

        if delta.value.is_hash() {
            // TigerStyle: Postcondition - replica_state should have the hash
            #[cfg(debug_assertions)]
            {
                debug_assert!(
                    self.replica_state.replicated_keys.contains_key(&delta.key),
                    "Postcondition: key must exist after applying hash delta"
                );
            }

            // Apply hash delta
            if let Some(hash) = delta.value.get_hash() {
                let pairs: Vec<(crate::redis::SDS, crate::redis::SDS)> = hash
                    .iter()
                    .filter_map(|(field, lww)| {
                        lww.get()
                            .map(|v| (crate::redis::SDS::from_str(field), v.clone()))
                    })
                    .collect();
                if !pairs.is_empty() {
                    let cmd = Command::HSet(delta.key.clone(), pairs.clone());
                    self.executor.execute(&cmd);

                    // TigerStyle: Postcondition - executor should have the hash fields
                    #[cfg(debug_assertions)]
                    {
                        if let Some(executor_value) = self.executor.get_data().get(&delta.key) {
                            debug_assert!(
                                executor_value.as_hash().is_some(),
                                "Postcondition: executor must have hash after HSet"
                            );
                        }
                    }
                }
                // Handle tombstoned fields by deleting them
                let tombstones: Vec<crate::redis::SDS> = hash
                    .iter()
                    .filter(|(_, lww)| lww.tombstone)
                    .map(|(field, _)| crate::redis::SDS::from_str(field))
                    .collect();
                if !tombstones.is_empty() {
                    let cmd = Command::HDel(delta.key.clone(), tombstones.clone());
                    self.executor.execute(&cmd);

                    // TigerStyle: Postcondition - tombstoned fields should be removed
                    #[cfg(debug_assertions)]
                    {
                        if let Some(executor_value) = self.executor.get_data().get(&delta.key) {
                            if let Some(exec_hash) = executor_value.as_hash() {
                                for tombstone_field in &tombstones {
                                    debug_assert!(exec_hash.get(tombstone_field).is_none(),
                                        "Postcondition: tombstoned field must be deleted from executor");
                                }
                            }
                        }
                    }
                }
            }
        } else if let Some(value) = delta.value.get() {
            if let Some(expiry_ms) = delta.value.expiry_ms {
                let seconds = (expiry_ms / 1000) as i64;
                let cmd = Command::setex(delta.key.clone(), seconds, value.clone());
                self.executor.execute(&cmd);
            } else {
                let cmd = Command::set(delta.key.clone(), value.clone());
                self.executor.execute(&cmd);
            }
        } else if delta.value.is_tombstone() {
            let cmd = Command::del(delta.key.clone());
            self.executor.execute(&cmd);
        }
    }

    /// Verify invariants (VOPR pattern)
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Invariant: Keys in executor data should be consistent with replica state
        // (Not strictly enforced as executor may have non-replicated keys)
        debug_assert!(
            self.shard_id < 256,
            "Invariant: shard_id {} seems unreasonably high",
            self.shard_id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::ConsistencyLevel;

    #[tokio::test]
    async fn test_replicated_shard_actor_basic() {
        let handle = ReplicatedShardActor::spawn(ReplicaId::new(1), ConsistencyLevel::Eventual, 0);

        // Execute SET
        let (result, delta) = handle
            .execute(Command::set(
                "key1".to_string(),
                crate::redis::SDS::from_str("value1"),
            ))
            .await;

        assert!(matches!(result, RespValue::SimpleString(_)));
        assert!(delta.is_some());

        // Execute GET
        let result = handle
            .execute_readonly(Command::Get("key1".to_string()))
            .await;
        assert!(matches!(result, RespValue::BulkString(Some(_))));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_replicated_shard_actor_drain_deltas() {
        let handle = ReplicatedShardActor::spawn(ReplicaId::new(1), ConsistencyLevel::Eventual, 0);

        // Execute some writes
        handle
            .execute(Command::set(
                "k1".to_string(),
                crate::redis::SDS::from_str("v1"),
            ))
            .await;
        handle
            .execute(Command::set(
                "k2".to_string(),
                crate::redis::SDS::from_str("v2"),
            ))
            .await;

        // Drain deltas
        let deltas = handle.drain_pending_deltas().await;
        assert_eq!(deltas.len(), 2);

        // Drain again should be empty
        let deltas = handle.drain_pending_deltas().await;
        assert!(deltas.is_empty());

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_replicated_shard_actor_apply_remote_delta() {
        let handle = ReplicatedShardActor::spawn(ReplicaId::new(1), ConsistencyLevel::Eventual, 0);

        // Create a delta as if from another replica
        let remote_replica = ReplicaId::new(2);
        let mut clock = crate::replication::lattice::LamportClock::new(remote_replica);
        clock.time = 100; // Advance clock to simulate remote write
        let delta = ReplicationDelta::new(
            "remote_key".to_string(),
            crate::replication::state::ReplicatedValue::with_value(
                crate::redis::SDS::from_str("remote_value"),
                clock,
            ),
            remote_replica,
        );

        // Apply it
        handle.apply_remote_delta(delta);

        // Small delay for message processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify it's there
        let result = handle
            .execute_readonly(Command::Get("remote_key".to_string()))
            .await;
        assert!(matches!(result, RespValue::BulkString(Some(_))));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_replicated_shard_actor_snapshot() {
        let handle = ReplicatedShardActor::spawn(ReplicaId::new(1), ConsistencyLevel::Eventual, 0);

        // Execute some writes
        handle
            .execute(Command::set(
                "k1".to_string(),
                crate::redis::SDS::from_str("v1"),
            ))
            .await;
        handle
            .execute(Command::set(
                "k2".to_string(),
                crate::redis::SDS::from_str("v2"),
            ))
            .await;

        // Get snapshot
        let snapshot = handle.get_snapshot().await;
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.contains_key("k1"));
        assert!(snapshot.contains_key("k2"));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_replicated_shard_actor_shutdown() {
        let handle = ReplicatedShardActor::spawn(ReplicaId::new(1), ConsistencyLevel::Eventual, 0);

        assert!(handle.is_running());
        handle.shutdown().await;

        // Give it a moment
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert!(!handle.is_running());
    }
}
