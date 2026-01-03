use super::state::ReplicationDelta;
use super::config::ReplicationConfig;
use super::lattice::ReplicaId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GossipMessage {
    DeltaBatch {
        source_replica: ReplicaId,
        deltas: Vec<ReplicationDelta>,
        epoch: u64,
    },
    SyncRequest {
        source_replica: ReplicaId,
        known_versions: HashMap<String, u64>,
    },
    SyncResponse {
        source_replica: ReplicaId,
        deltas: Vec<ReplicationDelta>,
    },
    Heartbeat {
        source_replica: ReplicaId,
        epoch: u64,
    },
}

impl GossipMessage {
    pub fn new_delta_batch(source: ReplicaId, deltas: Vec<ReplicationDelta>, epoch: u64) -> Self {
        GossipMessage::DeltaBatch {
            source_replica: source,
            deltas,
            epoch,
        }
    }

    pub fn new_heartbeat(source: ReplicaId, epoch: u64) -> Self {
        GossipMessage::Heartbeat {
            source_replica: source,
            epoch,
        }
    }

    pub fn serialize(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

pub type GossipSender = mpsc::UnboundedSender<GossipMessage>;
pub type GossipReceiver = mpsc::UnboundedReceiver<GossipMessage>;

pub fn create_gossip_channel() -> (GossipSender, GossipReceiver) {
    mpsc::unbounded_channel()
}

#[derive(Debug)]
pub struct GossipState {
    pub replica_id: ReplicaId,
    pub epoch: u64,
    pub config: ReplicationConfig,
    pub outbound_queue: Vec<GossipMessage>,
}

impl GossipState {
    pub fn new(config: ReplicationConfig) -> Self {
        GossipState {
            replica_id: ReplicaId::new(config.replica_id),
            epoch: 0,
            config,
            outbound_queue: Vec::new(),
        }
    }

    pub fn advance_epoch(&mut self) {
        self.epoch += 1;
    }

    pub fn queue_deltas(&mut self, deltas: Vec<ReplicationDelta>) {
        if !deltas.is_empty() {
            let msg = GossipMessage::new_delta_batch(self.replica_id, deltas, self.epoch);
            self.outbound_queue.push(msg);
        }
    }

    pub fn queue_heartbeat(&mut self) {
        let msg = GossipMessage::new_heartbeat(self.replica_id, self.epoch);
        self.outbound_queue.push(msg);
    }

    pub fn drain_outbound(&mut self) -> Vec<GossipMessage> {
        std::mem::take(&mut self.outbound_queue)
    }
}
