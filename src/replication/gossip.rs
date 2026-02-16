use super::config::ReplicationConfig;
use super::gossip_router::GossipRouter;
use super::lattice::ReplicaId;
use super::state::ReplicationDelta;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GossipMessage {
    /// Broadcast delta batch - sent to all peers (full replication mode)
    DeltaBatch {
        source_replica: ReplicaId,
        deltas: Vec<ReplicationDelta>,
        epoch: u64,
    },
    /// Targeted delta batch - sent to specific replica (selective gossip mode)
    TargetedDelta {
        source_replica: ReplicaId,
        target_replica: ReplicaId,
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

    /// Create a targeted delta message for selective gossip
    pub fn new_targeted_delta(
        source: ReplicaId,
        target: ReplicaId,
        deltas: Vec<ReplicationDelta>,
        epoch: u64,
    ) -> Self {
        GossipMessage::TargetedDelta {
            source_replica: source,
            target_replica: target,
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

    /// Get source replica ID from any message type
    pub fn source_replica(&self) -> ReplicaId {
        match self {
            GossipMessage::DeltaBatch { source_replica, .. } => *source_replica,
            GossipMessage::TargetedDelta { source_replica, .. } => *source_replica,
            GossipMessage::SyncRequest { source_replica, .. } => *source_replica,
            GossipMessage::SyncResponse { source_replica, .. } => *source_replica,
            GossipMessage::Heartbeat { source_replica, .. } => *source_replica,
        }
    }

    /// Extract deltas from delta-carrying message types
    pub fn into_deltas(self) -> Option<Vec<ReplicationDelta>> {
        match self {
            GossipMessage::DeltaBatch { deltas, .. } => Some(deltas),
            GossipMessage::TargetedDelta { deltas, .. } => Some(deltas),
            GossipMessage::SyncResponse { deltas, .. } => Some(deltas),
            _ => None,
        }
    }

    /// Check if this message is a delta carrier (DeltaBatch or TargetedDelta)
    pub fn is_delta_message(&self) -> bool {
        matches!(
            self,
            GossipMessage::DeltaBatch { .. } | GossipMessage::TargetedDelta { .. }
        )
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

/// Outbound message with routing information
#[derive(Debug, Clone)]
pub struct RoutedMessage {
    /// Target replica for this message (None = broadcast to all)
    pub target: Option<ReplicaId>,
    /// The gossip message
    pub message: GossipMessage,
}

impl RoutedMessage {
    pub fn broadcast(message: GossipMessage) -> Self {
        RoutedMessage {
            target: None,
            message,
        }
    }

    pub fn targeted(target: ReplicaId, message: GossipMessage) -> Self {
        RoutedMessage {
            target: Some(target),
            message,
        }
    }
}

#[derive(Debug)]
pub struct GossipState {
    pub replica_id: ReplicaId,
    pub epoch: u64,
    pub config: ReplicationConfig,
    /// Outbound queue with routing information
    pub outbound_queue: Vec<RoutedMessage>,
    /// Optional gossip router for selective gossip (partitioned mode)
    gossip_router: Option<GossipRouter>,
}

impl GossipState {
    /// Verify all invariants hold for this gossip state
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Invariant 1: replica_id must match config
        debug_assert_eq!(
            self.replica_id.0, self.config.replica_id,
            "Invariant violated: replica_id mismatch"
        );

        // Invariant 2: All targeted messages must have valid target
        for msg in &self.outbound_queue {
            if let Some(target) = msg.target {
                // Target should not be self
                debug_assert_ne!(
                    target, self.replica_id,
                    "Invariant violated: targeted message to self"
                );
            }
        }

        // Invariant 3: All outbound messages must have matching source replica
        for routed in &self.outbound_queue {
            debug_assert_eq!(
                routed.message.source_replica(),
                self.replica_id,
                "Invariant violated: outbound message source doesn't match replica_id"
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn verify_invariants(&self) {}

    pub fn new(config: ReplicationConfig) -> Self {
        GossipState {
            replica_id: ReplicaId::new(config.replica_id),
            epoch: 0,
            config,
            outbound_queue: Vec::new(),
            gossip_router: None,
        }
    }

    /// Create with a gossip router for selective gossip
    pub fn with_router(config: ReplicationConfig, router: GossipRouter) -> Self {
        GossipState {
            replica_id: ReplicaId::new(config.replica_id),
            epoch: 0,
            config,
            outbound_queue: Vec::new(),
            gossip_router: Some(router),
        }
    }

    /// Set or update the gossip router
    pub fn set_router(&mut self, router: GossipRouter) {
        self.gossip_router = Some(router);
    }

    pub fn advance_epoch(&mut self) {
        self.epoch += 1;
    }

    /// Queue deltas for gossip - uses selective routing if router is configured
    pub fn queue_deltas(&mut self, deltas: Vec<ReplicationDelta>) {
        if deltas.is_empty() {
            return;
        }

        if let Some(ref router) = self.gossip_router {
            if router.is_selective() {
                // Selective gossip: route deltas to specific replicas
                let routing_table = router.route_deltas(deltas);
                for (target_replica, target_deltas) in routing_table {
                    if !target_deltas.is_empty() {
                        let msg = GossipMessage::new_targeted_delta(
                            self.replica_id,
                            target_replica,
                            target_deltas,
                            self.epoch,
                        );
                        self.outbound_queue
                            .push(RoutedMessage::targeted(target_replica, msg));
                    }
                }
                return;
            }
        }

        // Fallback: broadcast to all peers
        let msg = GossipMessage::new_delta_batch(self.replica_id, deltas, self.epoch);
        self.outbound_queue.push(RoutedMessage::broadcast(msg));
    }

    /// Queue deltas using broadcast (ignore router)
    pub fn queue_deltas_broadcast(&mut self, deltas: Vec<ReplicationDelta>) {
        if !deltas.is_empty() {
            let msg = GossipMessage::new_delta_batch(self.replica_id, deltas, self.epoch);
            self.outbound_queue.push(RoutedMessage::broadcast(msg));
        }
    }

    pub fn queue_heartbeat(&mut self) {
        let msg = GossipMessage::new_heartbeat(self.replica_id, self.epoch);
        self.outbound_queue.push(RoutedMessage::broadcast(msg));
    }

    pub fn drain_outbound(&mut self) -> Vec<RoutedMessage> {
        std::mem::take(&mut self.outbound_queue)
    }

    /// Check if selective gossip is active
    pub fn is_selective(&self) -> bool {
        self.gossip_router
            .as_ref()
            .map(|r| r.is_selective())
            .unwrap_or(false)
    }

    /// Get reference to the gossip router (if configured)
    pub fn router(&self) -> Option<&GossipRouter> {
        self.gossip_router.as_ref()
    }
}
