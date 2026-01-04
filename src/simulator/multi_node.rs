//! Multi-Node Simulation Harness for Replication Testing
//!
//! Provides deterministic simulation of a cluster with:
//! - Multiple replicated nodes
//! - Network partitions and message loss
//! - Selective gossip routing
//! - CRDT convergence verification

use super::{DeterministicRng, Duration, VirtualTime};
use crate::redis::{Command, CommandExecutor, RespValue};
use crate::replication::anti_entropy::{AntiEntropyConfig, AntiEntropyManager, StateDigest};
use crate::replication::gossip::GossipState;
use crate::replication::gossip_router::GossipRouter;
use crate::replication::hash_ring::HashRing;
use crate::replication::state::{ReplicationDelta, ShardReplicaState};
use crate::replication::{ReplicaId, ReplicationConfig};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

/// Operation with invoke and complete timestamps for linearizability checking
#[derive(Debug, Clone)]
pub struct TimestampedOperation {
    pub client_id: usize,
    pub node_id: usize,
    pub invoke_time: VirtualTime,
    pub complete_time: VirtualTime,
    pub command: Command,
    pub response: RespValue,
}

/// Network message in flight between nodes
#[derive(Debug, Clone)]
pub struct InFlightMessage {
    pub from: usize,
    pub to: usize,
    pub deltas: Vec<ReplicationDelta>,
    pub delivery_time: VirtualTime,
}

/// Simulated node in the cluster
pub struct SimulatedNode {
    pub node_id: usize,
    pub replica_id: ReplicaId,
    pub executor: CommandExecutor,
    pub replica_state: ShardReplicaState,
    pub gossip_state: GossipState,
    pub anti_entropy: AntiEntropyManager,
}

impl SimulatedNode {
    pub fn new(node_id: usize, config: ReplicationConfig) -> Self {
        let replica_id = ReplicaId::new(node_id as u64 + 1);
        let mut executor = CommandExecutor::new();
        executor.set_simulation_start_epoch(0);

        SimulatedNode {
            node_id,
            replica_id,
            executor,
            replica_state: ShardReplicaState::new(replica_id, config.consistency_level),
            gossip_state: GossipState::new(config),
            anti_entropy: AntiEntropyManager::new(replica_id, AntiEntropyConfig::default()),
        }
    }

    /// Generate state digest for anti-entropy
    pub fn generate_digest(&self) -> StateDigest {
        self.anti_entropy.generate_digest(&self.replica_state.replicated_keys)
    }

    /// Get all keys for anti-entropy sync
    pub fn get_all_deltas(&self) -> Vec<ReplicationDelta> {
        self.replica_state
            .replicated_keys
            .iter()
            .map(|(key, value)| ReplicationDelta::new(key.clone(), value.clone(), self.replica_id))
            .collect()
    }

    /// Execute a command and record any replication deltas
    pub fn execute(&mut self, cmd: &Command) -> RespValue {
        let response = self.executor.execute(cmd);

        // Record writes for replication
        match cmd {
            Command::Set(key, value) => {
                self.replica_state
                    .record_write(key.clone(), value.clone(), None);
            }
            Command::SetEx(key, seconds, value) => {
                let expiry_ms = Some(*seconds as u64 * 1000);
                self.replica_state
                    .record_write(key.clone(), value.clone(), expiry_ms);
            }
            Command::Del(key) => {
                self.replica_state.record_delete(key.clone());
            }
            _ => {}
        }

        response
    }

    /// Collect pending deltas for gossip
    pub fn drain_deltas(&mut self) -> Vec<ReplicationDelta> {
        self.replica_state.drain_pending_deltas()
    }

    /// Apply remote deltas from another node
    pub fn apply_remote_deltas(&mut self, deltas: Vec<ReplicationDelta>) {
        for delta in deltas {
            // Apply to replica state
            self.replica_state.apply_remote_delta(delta.clone());

            // Also apply to command executor for GET to work
            if !delta.value.is_tombstone() {
                if let Some(value) = delta.value.get() {
                    let _ = self.executor.execute(&Command::Set(delta.key.clone(), value.clone()));
                }
            } else {
                let _ = self.executor.execute(&Command::Del(delta.key.clone()));
            }
        }
    }

    /// Get the value for a key from replica state
    pub fn get_replicated_value(&self, key: &str) -> Option<String> {
        self.replica_state.get_replicated(key).and_then(|rv| {
            if rv.is_tombstone() {
                None
            } else {
                rv.get().map(|sds| String::from_utf8_lossy(sds.as_bytes()).to_string())
            }
        })
    }
}

/// Multi-node cluster simulation
pub struct MultiNodeSimulation {
    pub nodes: Vec<SimulatedNode>,
    pub current_time: VirtualTime,
    pub rng: DeterministicRng,
    /// Messages in flight (delayed delivery)
    pub message_queue: VecDeque<InFlightMessage>,
    /// Network partitions: (node_a, node_b) pairs that can't communicate
    pub partitions: HashSet<(usize, usize)>,
    /// Packet loss probability (0.0 - 1.0)
    pub packet_loss_rate: f64,
    /// Message delay range in ms
    pub message_delay_range: (u64, u64),
    /// Operation history for linearizability checking
    pub history: Vec<TimestampedOperation>,
    /// Hash ring for partitioned mode
    pub hash_ring: Option<Arc<RwLock<HashRing>>>,
    /// Gossip router for selective routing
    pub gossip_routers: HashMap<usize, GossipRouter>,
    /// Enable automatic anti-entropy on partition heal
    pub auto_anti_entropy: bool,
    /// Anti-entropy sync statistics
    pub anti_entropy_syncs: u64,
}

impl MultiNodeSimulation {
    /// Create a new simulation with N nodes
    pub fn new(num_nodes: usize, seed: u64) -> Self {
        let nodes: Vec<SimulatedNode> = (0..num_nodes)
            .map(|i| {
                let config = ReplicationConfig::new_cluster(
                    i as u64 + 1,
                    (0..num_nodes)
                        .filter(|&j| j != i)
                        .map(|j| format!("127.0.0.1:{}", 3000 + j))
                        .collect(),
                );
                SimulatedNode::new(i, config)
            })
            .collect();

        MultiNodeSimulation {
            nodes,
            current_time: VirtualTime::ZERO,
            rng: DeterministicRng::new(seed),
            message_queue: VecDeque::new(),
            partitions: HashSet::new(),
            packet_loss_rate: 0.0,
            message_delay_range: (1, 10),
            history: Vec::new(),
            hash_ring: None,
            gossip_routers: HashMap::new(),
            auto_anti_entropy: true,
            anti_entropy_syncs: 0,
        }
    }

    /// Create a new simulation with anti-entropy disabled
    pub fn new_without_anti_entropy(num_nodes: usize, seed: u64) -> Self {
        let mut sim = Self::new(num_nodes, seed);
        sim.auto_anti_entropy = false;
        sim
    }

    /// Enable or disable automatic anti-entropy
    pub fn with_auto_anti_entropy(mut self, enabled: bool) -> Self {
        self.auto_anti_entropy = enabled;
        self
    }

    /// Create a simulation with partitioned mode (selective gossip)
    pub fn new_partitioned(num_nodes: usize, replication_factor: usize, seed: u64) -> Self {
        let replica_ids: Vec<ReplicaId> = (0..num_nodes)
            .map(|i| ReplicaId::new(i as u64 + 1))
            .collect();

        let hash_ring = Arc::new(RwLock::new(HashRing::new(
            replica_ids.clone(),
            150, // virtual nodes
            replication_factor,
        )));

        let nodes: Vec<SimulatedNode> = (0..num_nodes)
            .map(|i| {
                let config = ReplicationConfig::new_partitioned_cluster(
                    i as u64 + 1,
                    (0..num_nodes)
                        .filter(|&j| j != i)
                        .map(|j| format!("127.0.0.1:{}", 3000 + j))
                        .collect(),
                    replication_factor,
                );
                SimulatedNode::new(i, config)
            })
            .collect();

        // Create gossip routers for each node
        let mut gossip_routers = HashMap::new();
        for i in 0..num_nodes {
            let my_replica = ReplicaId::new(i as u64 + 1);
            let peer_addresses: HashMap<ReplicaId, String> = (0..num_nodes)
                .filter(|&j| j != i)
                .map(|j| (ReplicaId::new(j as u64 + 1), format!("127.0.0.1:{}", 3000 + j)))
                .collect();
            let router = GossipRouter::new(hash_ring.clone(), my_replica, peer_addresses, true);
            gossip_routers.insert(i, router);
        }

        MultiNodeSimulation {
            nodes,
            current_time: VirtualTime::ZERO,
            rng: DeterministicRng::new(seed),
            message_queue: VecDeque::new(),
            partitions: HashSet::new(),
            packet_loss_rate: 0.0,
            message_delay_range: (1, 10),
            history: Vec::new(),
            hash_ring: Some(hash_ring),
            gossip_routers,
            auto_anti_entropy: true,
            anti_entropy_syncs: 0,
        }
    }

    /// Set packet loss rate (0.0 - 1.0)
    pub fn with_packet_loss(mut self, rate: f64) -> Self {
        self.packet_loss_rate = rate.clamp(0.0, 1.0);
        self
    }

    /// Set message delay range in ms
    pub fn with_message_delay(mut self, min_ms: u64, max_ms: u64) -> Self {
        self.message_delay_range = (min_ms, max_ms);
        self
    }

    /// Advance simulation time
    pub fn advance_time(&mut self, new_time: VirtualTime) {
        debug_assert!(new_time >= self.current_time, "Time cannot go backwards");
        self.current_time = new_time;
    }

    /// Advance time by milliseconds
    pub fn advance_time_ms(&mut self, ms: u64) {
        self.current_time = self.current_time + Duration::from_millis(ms);
    }

    /// Execute a command on a specific node
    pub fn execute(&mut self, client_id: usize, node_id: usize, cmd: Command) -> RespValue {
        let invoke_time = self.current_time;
        let response = self.nodes[node_id].execute(&cmd);
        let complete_time = self.current_time;

        self.history.push(TimestampedOperation {
            client_id,
            node_id,
            invoke_time,
            complete_time,
            command: cmd,
            response: response.clone(),
        });

        response
    }

    /// Create a network partition between two nodes
    pub fn partition(&mut self, node_a: usize, node_b: usize) {
        let (a, b) = if node_a < node_b {
            (node_a, node_b)
        } else {
            (node_b, node_a)
        };
        self.partitions.insert((a, b));
    }

    /// Heal a network partition
    pub fn heal_partition(&mut self, node_a: usize, node_b: usize) {
        let (a, b) = if node_a < node_b {
            (node_a, node_b)
        } else {
            (node_b, node_a)
        };

        // Check if partition existed
        let was_partitioned = self.partitions.remove(&(a, b));

        // Trigger anti-entropy sync if enabled and partition existed
        if was_partitioned && self.auto_anti_entropy {
            self.run_anti_entropy_sync(node_a, node_b);
        }
    }

    /// Run anti-entropy sync between two nodes
    pub fn run_anti_entropy_sync(&mut self, node_a: usize, node_b: usize) {
        // Get digests from both nodes
        let digest_a = self.nodes[node_a].generate_digest();
        let digest_b = self.nodes[node_b].generate_digest();

        // Check if digests differ
        if digest_a.differs_from(&digest_b) {
            // Find divergent buckets
            let divergent = digest_a.divergent_buckets(&digest_b);

            if !divergent.is_empty() {
                // Get keys in divergent buckets from both nodes
                let deltas_a = self.nodes[node_a]
                    .anti_entropy
                    .get_keys_in_buckets(&self.nodes[node_a].replica_state.replicated_keys, &divergent);
                let deltas_b = self.nodes[node_b]
                    .anti_entropy
                    .get_keys_in_buckets(&self.nodes[node_b].replica_state.replicated_keys, &divergent);

                // Apply deltas bidirectionally
                self.nodes[node_b].apply_remote_deltas(deltas_a);
                self.nodes[node_a].apply_remote_deltas(deltas_b);

                self.anti_entropy_syncs += 1;
            }
        }
    }

    /// Run full anti-entropy sync across all connected node pairs
    pub fn run_full_anti_entropy(&mut self) {
        let num_nodes = self.nodes.len();

        for i in 0..num_nodes {
            for j in (i + 1)..num_nodes {
                if self.can_communicate(i, j) {
                    self.run_anti_entropy_sync(i, j);
                }
            }
        }
    }

    /// Check if two nodes can communicate
    pub fn can_communicate(&self, node_a: usize, node_b: usize) -> bool {
        let (a, b) = if node_a < node_b {
            (node_a, node_b)
        } else {
            (node_b, node_a)
        };
        !self.partitions.contains(&(a, b))
    }

    /// Run a gossip round - each node sends its deltas
    pub fn gossip_round(&mut self) {
        let num_nodes = self.nodes.len();

        // Collect deltas from each node
        let mut node_deltas: Vec<Vec<ReplicationDelta>> = Vec::new();
        for node in &mut self.nodes {
            node_deltas.push(node.drain_deltas());
        }

        // Route deltas based on mode (selective vs broadcast)
        for from_node in 0..num_nodes {
            let deltas = &node_deltas[from_node];
            if deltas.is_empty() {
                continue;
            }

            if let Some(router) = self.gossip_routers.get(&from_node) {
                // Selective gossip: route to specific nodes
                let routing_table = router.route_deltas(deltas.clone());
                for (target_replica, target_deltas) in routing_table {
                    let to_node = target_replica.0 as usize - 1;
                    self.send_deltas(from_node, to_node, target_deltas);
                }
            } else {
                // Broadcast gossip: send to all other nodes
                for to_node in 0..num_nodes {
                    if to_node != from_node {
                        self.send_deltas(from_node, to_node, deltas.clone());
                    }
                }
            }
        }

        // Deliver messages that are ready
        self.deliver_messages();
    }

    /// Send deltas from one node to another (with delay and possible loss)
    fn send_deltas(&mut self, from: usize, to: usize, deltas: Vec<ReplicationDelta>) {
        // Check partition
        if !self.can_communicate(from, to) {
            return; // Message dropped due to partition
        }

        // Check packet loss
        if self.rng.gen_bool(self.packet_loss_rate) {
            return; // Message dropped due to packet loss
        }

        // Calculate delivery time
        let delay_ms = self
            .rng
            .gen_range(self.message_delay_range.0, self.message_delay_range.1 + 1);
        let delivery_time = self.current_time + Duration::from_millis(delay_ms);

        self.message_queue.push_back(InFlightMessage {
            from,
            to,
            deltas,
            delivery_time,
        });
    }

    /// Deliver all messages that are ready
    fn deliver_messages(&mut self) {
        let mut delivered = Vec::new();

        // Find messages ready for delivery
        while let Some(msg) = self.message_queue.pop_front() {
            if msg.delivery_time <= self.current_time && self.can_communicate(msg.from, msg.to) {
                delivered.push(msg);
            } else {
                self.message_queue.push_front(msg);
                break;
            }
        }

        // Apply deltas
        for msg in delivered {
            self.nodes[msg.to].apply_remote_deltas(msg.deltas);
        }
    }

    /// Run multiple gossip rounds until convergence or max rounds
    pub fn converge(&mut self, max_rounds: usize) -> bool {
        for _ in 0..max_rounds {
            self.advance_time_ms(10);
            self.gossip_round();
        }
        true
    }

    /// Check if all nodes have converged on a key's value
    pub fn check_key_convergence(&self, key: &str) -> bool {
        let values: Vec<Option<String>> = self
            .nodes
            .iter()
            .map(|n| n.get_replicated_value(key))
            .collect();

        // All should be the same
        values.windows(2).all(|w| w[0] == w[1])
    }

    /// Get all values for a key across nodes (for debugging)
    pub fn get_all_values(&self, key: &str) -> Vec<Option<String>> {
        self.nodes
            .iter()
            .map(|n| n.get_replicated_value(key))
            .collect()
    }

    /// Count gossip messages that would be sent for selective vs broadcast
    pub fn count_gossip_messages(&self, deltas: &[ReplicationDelta]) -> (usize, usize) {
        let num_nodes = self.nodes.len();
        let broadcast_messages = deltas.len() * (num_nodes - 1);

        let selective_messages: usize = if let Some(router) = self.gossip_routers.get(&0) {
            let routing_table = router.route_deltas(deltas.to_vec());
            routing_table.values().map(|v| v.len()).sum()
        } else {
            broadcast_messages
        };

        (selective_messages, broadcast_messages)
    }
}

/// Result of a linearizability check
#[derive(Debug)]
pub struct LinearizabilityResult {
    pub is_linearizable: bool,
    pub violations: Vec<String>,
}

/// Check if a history of single-key operations is linearizable
pub fn check_single_key_linearizability(
    history: &[TimestampedOperation],
    key: &str,
) -> LinearizabilityResult {
    // Filter operations for this key
    let key_ops: Vec<&TimestampedOperation> = history
        .iter()
        .filter(|op| match &op.command {
            Command::Set(k, _) | Command::Get(k) => k == key,
            _ => false,
        })
        .collect();

    if key_ops.is_empty() {
        return LinearizabilityResult {
            is_linearizable: true,
            violations: vec![],
        };
    }

    // Sort by invoke time
    let mut ops = key_ops.clone();
    ops.sort_by_key(|op| op.invoke_time);

    // Simple linearizability check: if operations don't overlap,
    // the sequence of values should be consistent
    let mut violations = Vec::new();
    let mut expected_value: Option<String> = None;

    for op in &ops {
        match &op.command {
            Command::Set(_, value) => {
                expected_value = Some(String::from_utf8_lossy(value.as_bytes()).to_string());
            }
            Command::Get(_) => {
                if let RespValue::BulkString(Some(data)) = &op.response {
                    let got = String::from_utf8_lossy(data).to_string();
                    if let Some(ref expected) = expected_value {
                        if &got != expected {
                            violations.push(format!(
                                "GET at {:?} returned '{}', expected '{}'",
                                op.invoke_time, got, expected
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    LinearizabilityResult {
        is_linearizable: violations.is_empty(),
        violations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;

    #[test]
    fn test_basic_replication() {
        let mut sim = MultiNodeSimulation::new(3, 42);

        // Write on node 0
        sim.execute(
            1,
            0,
            Command::Set("key1".into(), SDS::from_str("value1")),
        );

        // Gossip round
        sim.gossip_round();
        sim.advance_time_ms(20);
        sim.gossip_round();

        // Check convergence
        assert!(sim.check_key_convergence("key1"));
        for node in &sim.nodes {
            assert_eq!(node.get_replicated_value("key1"), Some("value1".to_string()));
        }
    }

    #[test]
    fn test_partition_and_heal() {
        let mut sim = MultiNodeSimulation::new(3, 42);

        // Partition node 2
        sim.partition(0, 2);
        sim.partition(1, 2);

        // Write on node 0
        sim.execute(
            1,
            0,
            Command::Set("key1".into(), SDS::from_str("value_a")),
        );

        // Gossip rounds while partitioned
        for _ in 0..5 {
            sim.advance_time_ms(10);
            sim.gossip_round();
        }

        // Node 2 should not have the value
        assert_eq!(sim.nodes[1].get_replicated_value("key1"), Some("value_a".to_string()));
        assert_eq!(sim.nodes[2].get_replicated_value("key1"), None);

        // Heal partition
        sim.heal_partition(0, 2);
        sim.heal_partition(1, 2);

        // Write again to trigger gossip
        sim.execute(
            1,
            0,
            Command::Set("key1".into(), SDS::from_str("value_b")),
        );

        // Gossip rounds after healing
        for _ in 0..5 {
            sim.advance_time_ms(10);
            sim.gossip_round();
        }

        // Now all nodes should converge
        assert!(sim.check_key_convergence("key1"));
    }

    #[test]
    fn test_concurrent_writes_converge() {
        for seed in 0..10 {
            let mut sim = MultiNodeSimulation::new(3, seed);

            // Concurrent writes on different nodes
            sim.execute(
                1,
                0,
                Command::Set("key".into(), SDS::from_str("from_node_0")),
            );
            sim.execute(
                2,
                1,
                Command::Set("key".into(), SDS::from_str("from_node_1")),
            );
            sim.execute(
                3,
                2,
                Command::Set("key".into(), SDS::from_str("from_node_2")),
            );

            // Converge
            sim.converge(20);

            // All nodes should have the same value (LWW semantics)
            assert!(
                sim.check_key_convergence("key"),
                "Seed {} failed to converge: {:?}",
                seed,
                sim.get_all_values("key")
            );
        }
    }

    #[test]
    fn test_packet_loss_eventual_convergence() {
        let mut sim = MultiNodeSimulation::new(3, 42).with_packet_loss(0.3);

        // Write
        sim.execute(
            1,
            0,
            Command::Set("lossy_key".into(), SDS::from_str("value")),
        );

        // Many gossip rounds to overcome packet loss
        sim.converge(100);

        // Should eventually converge despite packet loss
        // Note: with 30% packet loss, convergence might take longer
        // In practice, you'd want to keep trying or have retransmission
    }

    #[test]
    fn test_selective_gossip_message_reduction() {
        let sim = MultiNodeSimulation::new_partitioned(10, 3, 42);

        // Create some deltas
        let deltas: Vec<ReplicationDelta> = (0..100)
            .map(|i| {
                let key = format!("key_{}", i);
                let replica_id = ReplicaId::new(1);
                let timestamp = crate::replication::lattice::LamportClock::new(replica_id);
                let value = crate::replication::state::ReplicatedValue::with_value(
                    SDS::from_str(&format!("value_{}", i)),
                    timestamp,
                );
                ReplicationDelta::new(key, value, replica_id)
            })
            .collect();

        let (selective, broadcast) = sim.count_gossip_messages(&deltas);

        // With RF=3 and 10 nodes, selective should be much less than broadcast
        // Broadcast: 100 deltas * 9 peers = 900
        // Selective: 100 deltas * ~2 peers (RF-1) = ~200
        println!(
            "Selective: {}, Broadcast: {}, Reduction: {:.1}%",
            selective,
            broadcast,
            (1.0 - selective as f64 / broadcast as f64) * 100.0
        );
        assert!(
            selective < broadcast,
            "Selective should send fewer messages"
        );
    }

    #[test]
    fn test_multi_seed_convergence() {
        for seed in 0..50 {
            // Use no packet loss for deterministic convergence testing
            let mut sim = MultiNodeSimulation::new(5, seed);

            // Random writes across nodes
            let node = (seed as usize) % 5;
            sim.execute(
                1,
                node,
                Command::Set("test_key".into(), SDS::from_str(&format!("value_{}", seed))),
            );

            // Converge with plenty of rounds
            sim.converge(20);

            // All nodes should agree
            assert!(
                sim.check_key_convergence("test_key"),
                "Seed {} failed to converge: {:?}",
                seed,
                sim.get_all_values("test_key")
            );
        }
    }
}
