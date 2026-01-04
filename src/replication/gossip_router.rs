//! Selective Gossip Router for Anna KVS-style Partitioned Replication
//!
//! Routes replication deltas only to nodes that are responsible for each key,
//! reducing network traffic from O(n) to O(replication_factor).

use super::config::ReplicationConfig;
use super::hash_ring::HashRing;
use super::lattice::ReplicaId;
use super::state::ReplicationDelta;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Routes deltas to appropriate nodes based on consistent hashing.
///
/// In full replication mode, broadcasts to all peers.
/// In partitioned mode, only sends to nodes responsible for each key.
#[derive(Debug)]
pub struct GossipRouter {
    /// Consistent hash ring for key-to-node mapping
    hash_ring: Arc<RwLock<HashRing>>,
    /// This node's replica ID
    my_replica: ReplicaId,
    /// Map from replica ID to network address
    peer_addresses: HashMap<ReplicaId, String>,
    /// Whether selective routing is enabled
    selective_mode: bool,
}

/// Result of routing deltas - maps target replica to deltas for that replica
pub type RoutingTable = HashMap<ReplicaId, Vec<ReplicationDelta>>;

/// Statistics about gossip routing efficiency
#[derive(Debug, Clone, Default)]
pub struct RoutingStats {
    /// Total deltas processed
    pub total_deltas: usize,
    /// Total delta-to-replica assignments (messages that would be sent)
    pub total_assignments: usize,
    /// Assignments saved by selective routing (vs full broadcast)
    pub assignments_saved: usize,
    /// Number of unique target replicas
    pub unique_targets: usize,
}

impl GossipRouter {
    /// Create a new gossip router
    pub fn new(
        hash_ring: Arc<RwLock<HashRing>>,
        my_replica: ReplicaId,
        peer_addresses: HashMap<ReplicaId, String>,
        selective_mode: bool,
    ) -> Self {
        GossipRouter {
            hash_ring,
            my_replica,
            peer_addresses,
            selective_mode,
        }
    }

    /// Create a router from replication config
    pub fn from_config(config: &ReplicationConfig, hash_ring: Arc<RwLock<HashRing>>) -> Self {
        let my_replica = ReplicaId::new(config.replica_id);

        // Build peer address map from config
        // Peers are expected to be in format "host:port" and numbered sequentially
        let mut peer_addresses = HashMap::new();
        for (i, addr) in config.peers.iter().enumerate() {
            // Assume peer IDs are sequential starting from 1, excluding self
            let peer_id = if i as u64 >= config.replica_id {
                i as u64 + 2 // Skip our own ID
            } else {
                i as u64 + 1
            };
            peer_addresses.insert(ReplicaId::new(peer_id), addr.clone());
        }

        GossipRouter {
            hash_ring,
            my_replica,
            peer_addresses,
            selective_mode: config.uses_selective_gossip(),
        }
    }

    /// Route deltas to their target nodes.
    ///
    /// In selective mode: only routes to nodes responsible for each key.
    /// In broadcast mode: routes all deltas to all peers.
    pub fn route_deltas(&self, deltas: Vec<ReplicationDelta>) -> RoutingTable {
        if self.selective_mode {
            self.route_selective(deltas)
        } else {
            self.route_broadcast(deltas)
        }
    }

    /// Selective routing: only send to responsible nodes
    fn route_selective(&self, deltas: Vec<ReplicationDelta>) -> RoutingTable {
        let ring = self.hash_ring.read().unwrap();
        let mut routing_table: RoutingTable = HashMap::new();

        for delta in deltas {
            // Get target replicas for this key (excluding self)
            let targets = ring.get_gossip_targets(&delta.key, self.my_replica);

            for target in targets {
                // Only include targets we have addresses for
                if self.peer_addresses.contains_key(&target) {
                    routing_table
                        .entry(target)
                        .or_default()
                        .push(delta.clone());
                }
            }
        }

        routing_table
    }

    /// Broadcast routing: send all deltas to all peers
    fn route_broadcast(&self, deltas: Vec<ReplicationDelta>) -> RoutingTable {
        let mut routing_table: RoutingTable = HashMap::new();

        for (peer_id, _) in &self.peer_addresses {
            if *peer_id != self.my_replica {
                routing_table.insert(*peer_id, deltas.clone());
            }
        }

        routing_table
    }

    /// Route deltas and calculate efficiency statistics
    pub fn route_with_stats(&self, deltas: Vec<ReplicationDelta>) -> (RoutingTable, RoutingStats) {
        let total_deltas = deltas.len();
        let peer_count = self.peer_addresses.len();

        let routing_table = self.route_deltas(deltas);

        // Calculate stats
        let total_assignments: usize = routing_table.values().map(|v| v.len()).sum();
        let unique_targets = routing_table.len();

        // In broadcast mode, we'd send total_deltas to each peer
        let broadcast_assignments = total_deltas * peer_count;
        let assignments_saved = broadcast_assignments.saturating_sub(total_assignments);

        let stats = RoutingStats {
            total_deltas,
            total_assignments,
            assignments_saved,
            unique_targets,
        };

        (routing_table, stats)
    }

    /// Get the network address for a replica
    pub fn get_peer_address(&self, replica: ReplicaId) -> Option<&String> {
        self.peer_addresses.get(&replica)
    }

    /// Get all peer replica IDs
    pub fn peer_ids(&self) -> impl Iterator<Item = &ReplicaId> {
        self.peer_addresses.keys()
    }

    /// Check if selective routing is enabled
    pub fn is_selective(&self) -> bool {
        self.selective_mode
    }

    /// Get this node's replica ID
    pub fn my_replica(&self) -> ReplicaId {
        self.my_replica
    }

    /// Update peer addresses (for dynamic membership)
    pub fn update_peer(&mut self, replica: ReplicaId, address: String) {
        self.peer_addresses.insert(replica, address);
    }

    /// Remove a peer (for dynamic membership)
    pub fn remove_peer(&mut self, replica: ReplicaId) {
        self.peer_addresses.remove(&replica);
    }

    /// Calculate expected message reduction ratio for selective gossip
    ///
    /// Returns (selective_msgs, broadcast_msgs, reduction_ratio)
    pub fn calculate_reduction_ratio(&self, sample_keys: &[&str]) -> (usize, usize, f64) {
        let ring = self.hash_ring.read().unwrap();
        let peer_count = self.peer_addresses.len();

        if peer_count == 0 {
            return (0, 0, 0.0);
        }

        let mut selective_msgs = 0;
        for key in sample_keys {
            let targets = ring.get_gossip_targets(key, self.my_replica);
            selective_msgs += targets.len();
        }

        let broadcast_msgs = sample_keys.len() * peer_count;
        let reduction = if broadcast_msgs > 0 {
            1.0 - (selective_msgs as f64 / broadcast_msgs as f64)
        } else {
            0.0
        };

        (selective_msgs, broadcast_msgs, reduction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_ring(num_nodes: u64) -> Arc<RwLock<HashRing>> {
        let nodes: Vec<ReplicaId> = (1..=num_nodes).map(ReplicaId::new).collect();
        Arc::new(RwLock::new(HashRing::new(nodes, 50, 3)))
    }

    fn create_test_router(num_nodes: u64, my_id: u64, selective: bool) -> GossipRouter {
        let ring = create_test_ring(num_nodes);
        let mut peer_addresses = HashMap::new();
        for i in 1..=num_nodes {
            if i != my_id {
                peer_addresses.insert(ReplicaId::new(i), format!("127.0.0.1:{}", 3000 + i));
            }
        }
        GossipRouter::new(ring, ReplicaId::new(my_id), peer_addresses, selective)
    }

    fn create_test_delta(key: &str) -> ReplicationDelta {
        use super::super::lattice::LamportClock;
        use super::super::state::ReplicatedValue;
        use crate::redis::SDS;

        let replica_id = ReplicaId::new(1);
        let timestamp = LamportClock::new(replica_id);
        let value = ReplicatedValue::with_value(SDS::from_str("test"), timestamp);
        ReplicationDelta::new(key.to_string(), value, replica_id)
    }

    #[test]
    fn test_broadcast_routing() {
        let router = create_test_router(5, 1, false);
        let deltas = vec![create_test_delta("key1"), create_test_delta("key2")];

        let routing = router.route_deltas(deltas);

        // Should send to all 4 other peers
        assert_eq!(routing.len(), 4);

        // Each peer should get both deltas
        for (_, peer_deltas) in &routing {
            assert_eq!(peer_deltas.len(), 2);
        }
    }

    #[test]
    fn test_selective_routing() {
        let router = create_test_router(5, 1, true);
        let deltas = vec![create_test_delta("key1")];

        let routing = router.route_deltas(deltas);

        // With RF=3 and 5 nodes, should route to at most 2 other nodes
        // (3 replicas total, minus self if responsible)
        assert!(routing.len() <= 3);

        // Total assignments should be less than broadcast (which would be 4)
        let total: usize = routing.values().map(|v| v.len()).sum();
        assert!(total <= 3);
    }

    #[test]
    fn test_routing_stats() {
        let router = create_test_router(10, 1, true);
        let deltas: Vec<_> = (0..100).map(|i| create_test_delta(&format!("key{}", i))).collect();

        let (_routing, stats) = router.route_with_stats(deltas);

        assert_eq!(stats.total_deltas, 100);
        assert!(stats.unique_targets <= 9); // At most 9 other nodes

        // Selective should save significant assignments vs broadcast
        // Broadcast: 100 deltas * 9 peers = 900
        // Selective: ~100 deltas * 2 peers (RF-1) = ~200
        assert!(stats.assignments_saved > 0);
        println!(
            "Selective: {} assignments, Broadcast would be: {}, Saved: {}",
            stats.total_assignments,
            stats.total_assignments + stats.assignments_saved,
            stats.assignments_saved
        );
    }

    #[test]
    fn test_reduction_ratio() {
        let router = create_test_router(10, 1, true);
        let keys: Vec<String> = (0..1000).map(|i| format!("key{}", i)).collect();
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();

        let (selective, broadcast, ratio) = router.calculate_reduction_ratio(&key_refs);

        // With 10 nodes, RF=3, we expect ~77% reduction
        // Broadcast: 1000 * 9 = 9000 messages
        // Selective: 1000 * ~2 = ~2000 messages (RF-1 on average)
        println!(
            "Selective: {}, Broadcast: {}, Reduction: {:.1}%",
            selective,
            broadcast,
            ratio * 100.0
        );

        assert!(ratio > 0.5, "Expected >50% reduction, got {:.1}%", ratio * 100.0);
    }

    #[test]
    fn test_single_node_cluster() {
        let router = create_test_router(1, 1, true);
        let deltas = vec![create_test_delta("key1")];

        let routing = router.route_deltas(deltas);

        // No peers to route to
        assert!(routing.is_empty());
    }

    #[test]
    fn test_peer_management() {
        let mut router = create_test_router(3, 1, true);

        assert!(router.get_peer_address(ReplicaId::new(2)).is_some());
        assert!(router.get_peer_address(ReplicaId::new(3)).is_some());

        router.remove_peer(ReplicaId::new(2));
        assert!(router.get_peer_address(ReplicaId::new(2)).is_none());

        router.update_peer(ReplicaId::new(4), "127.0.0.1:3004".to_string());
        assert!(router.get_peer_address(ReplicaId::new(4)).is_some());
    }
}
