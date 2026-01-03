use crate::redis::{Command, CommandExecutor, RespValue, SDS};
use crate::replication::{
    ReplicaId, ReplicationConfig, ConsistencyLevel,
    ReplicationDelta,
};
use crate::replication::state::ShardReplicaState;
use crate::replication::gossip::GossipState;
use crate::simulator::VirtualTime;
use parking_lot::RwLock;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const NUM_SHARDS: usize = 16;

fn hash_key(key: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % NUM_SHARDS
}

pub struct ReplicatedShard {
    executor: CommandExecutor,
    replica_state: ShardReplicaState,
}

impl ReplicatedShard {
    pub fn new(replica_id: ReplicaId, consistency_level: ConsistencyLevel) -> Self {
        ReplicatedShard {
            executor: CommandExecutor::new(),
            replica_state: ShardReplicaState::new(replica_id, consistency_level),
        }
    }

    pub fn execute(&mut self, cmd: &Command) -> (RespValue, Option<ReplicationDelta>) {
        let result = self.executor.execute(cmd);
        let delta = self.record_mutation_post_execute(cmd);
        (result, delta)
    }

    fn record_mutation_post_execute(&mut self, cmd: &Command) -> Option<ReplicationDelta> {
        match cmd {
            Command::Set(key, value) => {
                Some(self.replica_state.record_write(key.clone(), value.clone(), None))
            }
            Command::SetEx(key, seconds, value) => {
                let expiry_ms = (*seconds as u64) * 1000;
                Some(self.replica_state.record_write(key.clone(), value.clone(), Some(expiry_ms)))
            }
            Command::SetNx(key, value) => {
                if let Some(v) = self.executor.get_data().get(key) {
                    if v.as_string().is_some() {
                        return Some(self.replica_state.record_write(key.clone(), value.clone(), None));
                    }
                }
                None
            }
            Command::Del(key) => {
                self.replica_state.record_delete(key.clone())
            }
            Command::Incr(key) | Command::Decr(key) |
            Command::IncrBy(key, _) | Command::DecrBy(key, _) |
            Command::Append(key, _) | Command::GetSet(key, _) => {
                if let Some(value) = self.executor.get_data().get(key) {
                    if let Some(sds) = value.as_string() {
                        return Some(self.replica_state.record_write(key.clone(), sds.clone(), None));
                    }
                }
                None
            }
            Command::FlushDb | Command::FlushAll => {
                None
            }
            _ => None,
        }
    }

    pub fn apply_remote_delta(&mut self, delta: ReplicationDelta) {
        self.replica_state.apply_remote_delta(delta.clone());

        if let Some(value) = delta.value.get() {
            if let Some(expiry_ms) = delta.value.expiry_ms {
                let seconds = (expiry_ms / 1000) as i64;
                let cmd = Command::SetEx(delta.key.clone(), seconds, value.clone());
                self.executor.execute(&cmd);
            } else {
                let cmd = Command::Set(delta.key.clone(), value.clone());
                self.executor.execute(&cmd);
            }
        } else if delta.value.is_tombstone() {
            let cmd = Command::Del(delta.key.clone());
            self.executor.execute(&cmd);
        }
    }

    pub fn drain_pending_deltas(&mut self) -> Vec<ReplicationDelta> {
        self.replica_state.drain_pending_deltas()
    }

    pub fn evict_expired(&mut self, current_time: VirtualTime) -> usize {
        self.executor.evict_expired_direct(current_time)
    }
}

pub struct ReplicatedShardedState {
    shards: Vec<Arc<RwLock<ReplicatedShard>>>,
    config: ReplicationConfig,
    gossip_state: Arc<RwLock<GossipState>>,
}

impl ReplicatedShardedState {
    pub fn new(config: ReplicationConfig) -> Self {
        let replica_id = ReplicaId::new(config.replica_id);
        let consistency_level = config.consistency_level;

        let shards = (0..NUM_SHARDS)
            .map(|_| Arc::new(RwLock::new(ReplicatedShard::new(replica_id, consistency_level))))
            .collect();

        let gossip_state = Arc::new(RwLock::new(GossipState::new(config.clone())));

        ReplicatedShardedState {
            shards,
            config,
            gossip_state,
        }
    }

    pub fn execute(&self, cmd: Command) -> RespValue {
        if let Some(key) = cmd.get_primary_key() {
            let shard_idx = hash_key(&key);
            let mut shard = self.shards[shard_idx].write();
            let (result, delta) = shard.execute(&cmd);

            if let Some(delta) = delta {
                if self.config.enabled {
                    let mut gossip = self.gossip_state.write();
                    gossip.queue_deltas(vec![delta]);
                }
            }

            result
        } else {
            self.execute_global(cmd)
        }
    }

    fn execute_global(&self, cmd: Command) -> RespValue {
        match &cmd {
            Command::Ping => RespValue::SimpleString("PONG".to_string()),
            Command::FlushDb | Command::FlushAll => {
                for shard in &self.shards {
                    let mut s = shard.write();
                    s.executor.execute(&cmd);
                }
                RespValue::SimpleString("OK".to_string())
            }
            Command::MSet(pairs) => {
                for (key, value) in pairs {
                    let shard_idx = hash_key(key);
                    let mut shard = self.shards[shard_idx].write();
                    let set_cmd = Command::Set(key.clone(), value.clone());
                    shard.execute(&set_cmd);
                }
                RespValue::SimpleString("OK".to_string())
            }
            Command::MGet(keys) => {
                let mut results = Vec::new();
                for key in keys {
                    let shard_idx = hash_key(key);
                    let shard = self.shards[shard_idx].read();
                    let get_cmd = Command::Get(key.clone());
                    results.push(shard.executor.execute_readonly(&get_cmd));
                }
                RespValue::Array(Some(results))
            }
            Command::Exists(keys) => {
                let mut count = 0i64;
                for key in keys {
                    let shard_idx = hash_key(key);
                    let shard = self.shards[shard_idx].read();
                    let exists_cmd = Command::Exists(vec![key.clone()]);
                    if let RespValue::Integer(n) = shard.executor.execute_readonly(&exists_cmd) {
                        count += n;
                    }
                }
                RespValue::Integer(count)
            }
            Command::Keys(pattern) => {
                let mut all_keys = Vec::new();
                for shard in &self.shards {
                    let s = shard.read();
                    if let RespValue::Array(Some(keys)) = s.executor.execute_readonly(&cmd) {
                        all_keys.extend(keys);
                    }
                }
                RespValue::Array(Some(all_keys))
            }
            Command::Info => {
                let info = format!(
                    "# Replication\r\nrole:master\r\nreplica_id:{}\r\nconsistency_level:{:?}\r\nreplication_enabled:{}\r\nnum_shards:{}\r\n",
                    self.config.replica_id,
                    self.config.consistency_level,
                    self.config.enabled,
                    NUM_SHARDS
                );
                RespValue::BulkString(Some(info.into_bytes()))
            }
            _ => RespValue::Error("ERR unknown command".to_string()),
        }
    }

    pub fn apply_remote_deltas(&self, deltas: Vec<ReplicationDelta>) {
        for delta in deltas {
            let shard_idx = hash_key(&delta.key);
            let mut shard = self.shards[shard_idx].write();
            shard.apply_remote_delta(delta);
        }
    }

    pub fn collect_pending_deltas(&self) -> Vec<ReplicationDelta> {
        let mut all_deltas = Vec::new();
        for shard in &self.shards {
            let mut s = shard.write();
            all_deltas.extend(s.drain_pending_deltas());
        }
        all_deltas
    }

    pub fn evict_expired_all_shards(&self) -> usize {
        let current_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let current_time = VirtualTime::from_millis(current_time_ms);

        let mut total_evicted = 0;
        for shard in &self.shards {
            let mut s = shard.write();
            total_evicted += s.evict_expired(current_time);
        }
        total_evicted
    }

    pub fn get_gossip_state(&self) -> Arc<RwLock<GossipState>> {
        self.gossip_state.clone()
    }

    pub fn config(&self) -> &ReplicationConfig {
        &self.config
    }

    pub fn num_shards(&self) -> usize {
        NUM_SHARDS
    }
}

impl Clone for ReplicatedShardedState {
    fn clone(&self) -> Self {
        ReplicatedShardedState {
            shards: self.shards.clone(),
            config: self.config.clone(),
            gossip_state: self.gossip_state.clone(),
        }
    }
}
