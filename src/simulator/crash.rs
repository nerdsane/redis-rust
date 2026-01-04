//! Crash/Recovery Simulation for Deterministic Simulation Testing
//!
//! This module simulates node crashes and recoveries with:
//! - Checkpoint/restore semantics
//! - Partial state loss on crash
//! - Recovery timing control
//! - In-flight operation handling

use super::{HostId, VirtualTime};
use crate::buggify::{self, faults};
use crate::io::Rng;
use std::collections::HashMap;

/// State of a node in the simulation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeState {
    /// Node is running normally
    Running,
    /// Node has crashed
    Crashed {
        crash_time: VirtualTime,
        reason: CrashReason,
    },
    /// Node is in the process of recovering
    Recovering {
        recovery_start: VirtualTime,
        expected_completion: VirtualTime,
    },
}

/// Reason for a node crash
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrashReason {
    /// Intentional crash triggered by BUGGIFY
    BuggifyTriggered,
    /// Simulated power failure
    PowerFailure,
    /// Out of memory
    OutOfMemory,
    /// Network isolation (treated as crash from perspective of other nodes)
    NetworkIsolation,
    /// Explicit test-triggered crash
    TestTriggered,
}

/// Snapshot of node state for checkpoint/restore
#[derive(Debug, Clone)]
pub struct NodeSnapshot {
    /// Node identifier
    pub node_id: HostId,
    /// Timestamp when snapshot was taken
    pub snapshot_time: VirtualTime,
    /// Serialized state data
    pub state_data: Vec<u8>,
    /// In-flight operations at snapshot time
    pub pending_operations: Vec<PendingOperation>,
    /// Last acknowledged sequence number
    pub last_ack_seq: u64,
}

/// An operation that was in progress when crash occurred
#[derive(Debug, Clone)]
pub struct PendingOperation {
    pub operation_id: u64,
    pub operation_type: OperationType,
    pub start_time: VirtualTime,
    pub data: Vec<u8>,
}

/// Types of operations that can be in-flight
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationType {
    Read,
    Write,
    Gossip,
    Replication,
}

/// Configuration for crash simulation
#[derive(Debug, Clone)]
pub struct CrashConfig {
    /// Base probability of crash per operation (before BUGGIFY multiplier)
    pub base_crash_probability: f64,
    /// Minimum recovery time in milliseconds
    pub min_recovery_time_ms: u64,
    /// Maximum recovery time in milliseconds
    pub max_recovery_time_ms: u64,
    /// Probability of partial state loss on recovery
    pub partial_state_loss_probability: f64,
    /// Whether to enable automatic BUGGIFY-triggered crashes
    pub enable_buggify_crashes: bool,
}

impl Default for CrashConfig {
    fn default() -> Self {
        CrashConfig {
            base_crash_probability: 0.001, // 0.1%
            min_recovery_time_ms: 100,
            max_recovery_time_ms: 5000,
            partial_state_loss_probability: 0.1, // 10% chance of losing some pending writes
            enable_buggify_crashes: true,
        }
    }
}

/// Crash simulator managing node crashes and recoveries
pub struct CrashSimulator {
    /// Current state of each node
    node_states: HashMap<HostId, NodeState>,
    /// Checkpoints for each node (node_id -> list of snapshots)
    checkpoints: HashMap<HostId, Vec<NodeSnapshot>>,
    /// Configuration
    config: CrashConfig,
    /// Statistics
    stats: CrashStats,
}

/// Statistics about crashes and recoveries
#[derive(Debug, Clone, Default)]
pub struct CrashStats {
    pub total_crashes: u64,
    pub total_recoveries: u64,
    pub crashes_by_reason: HashMap<String, u64>,
    pub total_state_loss_events: u64,
    pub average_recovery_time_ms: f64,
}

impl CrashSimulator {
    /// Create a new crash simulator with default config
    pub fn new() -> Self {
        Self::with_config(CrashConfig::default())
    }

    /// Create a crash simulator with custom config
    pub fn with_config(config: CrashConfig) -> Self {
        CrashSimulator {
            node_states: HashMap::new(),
            checkpoints: HashMap::new(),
            config,
            stats: CrashStats::default(),
        }
    }

    /// Register a node with the crash simulator
    pub fn register_node(&mut self, node_id: HostId) {
        self.node_states.insert(node_id, NodeState::Running);
        self.checkpoints.insert(node_id, Vec::new());
    }

    /// Get current state of a node
    pub fn get_state(&self, node_id: HostId) -> Option<&NodeState> {
        self.node_states.get(&node_id)
    }

    /// Check if a node is currently running
    pub fn is_running(&self, node_id: HostId) -> bool {
        matches!(self.node_states.get(&node_id), Some(NodeState::Running))
    }

    /// Check if a node is crashed
    pub fn is_crashed(&self, node_id: HostId) -> bool {
        matches!(self.node_states.get(&node_id), Some(NodeState::Crashed { .. }))
    }

    /// Check if a node is recovering
    pub fn is_recovering(&self, node_id: HostId) -> bool {
        matches!(self.node_states.get(&node_id), Some(NodeState::Recovering { .. }))
    }

    /// Maybe crash a node based on BUGGIFY probability
    /// Returns true if node was crashed
    pub fn maybe_crash<R: Rng>(&mut self, rng: &mut R, node_id: HostId, time: VirtualTime) -> bool {
        if !self.config.enable_buggify_crashes {
            return false;
        }

        if !self.is_running(node_id) {
            return false;
        }

        // Use helper function instead of macro
        if check_buggify(rng, faults::process::CRASH) {
            self.crash_node(node_id, time, CrashReason::BuggifyTriggered);
            return true;
        }

        false
    }

    /// Explicitly crash a node
    pub fn crash_node(&mut self, node_id: HostId, time: VirtualTime, reason: CrashReason) {
        if !self.is_running(node_id) {
            return;
        }

        let reason_str = format!("{:?}", reason);
        self.node_states.insert(
            node_id,
            NodeState::Crashed {
                crash_time: time,
                reason,
            },
        );

        self.stats.total_crashes += 1;
        *self.stats.crashes_by_reason.entry(reason_str).or_insert(0) += 1;
    }

    /// Start recovery for a crashed node
    pub fn start_recovery<R: Rng>(
        &mut self,
        rng: &mut R,
        node_id: HostId,
        time: VirtualTime,
    ) -> Option<&NodeSnapshot> {
        if !self.is_crashed(node_id) {
            return None;
        }

        // Calculate recovery time
        let recovery_duration = rng.gen_range(
            self.config.min_recovery_time_ms,
            self.config.max_recovery_time_ms,
        );

        let expected_completion = VirtualTime(time.0 + recovery_duration);

        self.node_states.insert(
            node_id,
            NodeState::Recovering {
                recovery_start: time,
                expected_completion,
            },
        );

        // Return latest checkpoint if available
        self.checkpoints
            .get(&node_id)
            .and_then(|snapshots| snapshots.last())
    }

    /// Complete recovery for a node
    pub fn complete_recovery(&mut self, node_id: HostId, time: VirtualTime) -> bool {
        match self.node_states.get(&node_id) {
            Some(NodeState::Recovering {
                expected_completion,
                recovery_start,
            }) => {
                if time >= *expected_completion {
                    // Update stats
                    let recovery_time = time.0 - recovery_start.0;
                    let total_recoveries = self.stats.total_recoveries as f64;
                    self.stats.average_recovery_time_ms = (self.stats.average_recovery_time_ms
                        * total_recoveries
                        + recovery_time as f64)
                        / (total_recoveries + 1.0);
                    self.stats.total_recoveries += 1;

                    // Mark as running
                    self.node_states.insert(node_id, NodeState::Running);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Create a checkpoint for a node
    pub fn checkpoint(
        &mut self,
        node_id: HostId,
        time: VirtualTime,
        state_data: Vec<u8>,
        pending_ops: Vec<PendingOperation>,
        last_ack_seq: u64,
    ) {
        if !self.is_running(node_id) {
            return;
        }

        let snapshot = NodeSnapshot {
            node_id,
            snapshot_time: time,
            state_data,
            pending_operations: pending_ops,
            last_ack_seq,
        };

        if let Some(snapshots) = self.checkpoints.get_mut(&node_id) {
            // Keep only last 5 checkpoints to bound memory
            if snapshots.len() >= 5 {
                snapshots.remove(0);
            }
            snapshots.push(snapshot);
        }
    }

    /// Get the latest checkpoint for a node
    pub fn get_latest_checkpoint(&self, node_id: HostId) -> Option<&NodeSnapshot> {
        self.checkpoints
            .get(&node_id)
            .and_then(|snapshots| snapshots.last())
    }

    /// Simulate partial state loss during recovery
    /// Returns which pending operations were lost
    pub fn simulate_state_loss<R: Rng>(
        &mut self,
        rng: &mut R,
        snapshot: &NodeSnapshot,
    ) -> Vec<PendingOperation> {
        let mut lost_ops = Vec::new();

        // Check if partial state loss should occur
        let random_val = rng.gen_range(0, 1000) as f64 / 1000.0;
        if random_val < self.config.partial_state_loss_probability {
            self.stats.total_state_loss_events += 1;

            // Lose some pending operations (typically uncommitted writes)
            for op in &snapshot.pending_operations {
                // More likely to lose writes than reads
                let loss_chance = match op.operation_type {
                    OperationType::Write => 0.5,
                    OperationType::Replication => 0.3,
                    OperationType::Gossip => 0.2,
                    OperationType::Read => 0.1,
                };

                if rng.gen_bool(loss_chance) {
                    lost_ops.push(op.clone());
                }
            }
        }

        lost_ops
    }

    /// Get statistics
    pub fn stats(&self) -> &CrashStats {
        &self.stats
    }

    /// Get all crashed nodes
    pub fn crashed_nodes(&self) -> Vec<HostId> {
        self.node_states
            .iter()
            .filter_map(|(id, state)| {
                if matches!(state, NodeState::Crashed { .. }) {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get all recovering nodes
    pub fn recovering_nodes(&self) -> Vec<HostId> {
        self.node_states
            .iter()
            .filter_map(|(id, state)| {
                if matches!(state, NodeState::Recovering { .. }) {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Process time advancement - complete any recoveries that should finish
    pub fn advance_time(&mut self, time: VirtualTime) -> Vec<HostId> {
        let mut completed = Vec::new();

        let recovering: Vec<_> = self.recovering_nodes();
        for node_id in recovering {
            if self.complete_recovery(node_id, time) {
                completed.push(node_id);
            }
        }

        completed
    }
}

impl Default for CrashSimulator {
    fn default() -> Self {
        Self::new()
    }
}

// Helper function to avoid macro import issues
#[inline]
fn check_buggify<R: Rng>(rng: &mut R, fault_id: &str) -> bool {
    buggify::should_buggify(rng, fault_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buggify::FaultConfig;
    use crate::io::simulation::SimulatedRng;

    #[test]
    fn test_node_lifecycle() {
        let mut sim = CrashSimulator::new();
        let node = HostId(1);

        sim.register_node(node);
        assert!(sim.is_running(node));

        // Crash the node
        sim.crash_node(node, VirtualTime(1000), CrashReason::TestTriggered);
        assert!(sim.is_crashed(node));
        assert!(!sim.is_running(node));

        // Start recovery
        let mut rng = SimulatedRng::new(42);
        sim.start_recovery(&mut rng, node, VirtualTime(2000));
        assert!(sim.is_recovering(node));

        // Complete recovery after enough time
        sim.complete_recovery(node, VirtualTime(10000));
        assert!(sim.is_running(node));
    }

    #[test]
    fn test_checkpointing() {
        let mut sim = CrashSimulator::new();
        let node = HostId(1);

        sim.register_node(node);

        // Create checkpoint
        sim.checkpoint(
            node,
            VirtualTime(1000),
            vec![1, 2, 3],
            vec![PendingOperation {
                operation_id: 1,
                operation_type: OperationType::Write,
                start_time: VirtualTime(900),
                data: vec![4, 5, 6],
            }],
            100,
        );

        let checkpoint = sim.get_latest_checkpoint(node).unwrap();
        assert_eq!(checkpoint.state_data, vec![1, 2, 3]);
        assert_eq!(checkpoint.last_ack_seq, 100);
        assert_eq!(checkpoint.pending_operations.len(), 1);
    }

    #[test]
    fn test_stats_tracking() {
        let mut sim = CrashSimulator::new();
        let node = HostId(1);
        let mut rng = SimulatedRng::new(42);

        sim.register_node(node);

        // Crash a few times
        for i in 0..3 {
            sim.crash_node(node, VirtualTime(i * 1000), CrashReason::TestTriggered);
            sim.start_recovery(&mut rng, node, VirtualTime(i * 1000 + 100));
            sim.complete_recovery(node, VirtualTime(i * 1000 + 5000));
        }

        assert_eq!(sim.stats().total_crashes, 3);
        assert_eq!(sim.stats().total_recoveries, 3);
    }

    #[test]
    fn test_advance_time_completes_recovery() {
        let mut sim = CrashSimulator::with_config(CrashConfig {
            min_recovery_time_ms: 100,
            max_recovery_time_ms: 200,
            ..Default::default()
        });

        let node = HostId(1);
        let mut rng = SimulatedRng::new(42);

        sim.register_node(node);
        sim.crash_node(node, VirtualTime(1000), CrashReason::TestTriggered);
        sim.start_recovery(&mut rng, node, VirtualTime(2000));

        // Time hasn't advanced enough
        let completed = sim.advance_time(VirtualTime(2050));
        assert!(completed.is_empty());
        assert!(sim.is_recovering(node));

        // Now enough time has passed
        let completed = sim.advance_time(VirtualTime(3000));
        assert_eq!(completed, vec![node]);
        assert!(sim.is_running(node));
    }

    #[test]
    fn test_buggify_triggered_crash() {
        // Set up high crash probability for testing
        buggify::set_config(FaultConfig::chaos());

        let mut sim = CrashSimulator::new();
        let node = HostId(1);
        let mut rng = SimulatedRng::new(42);

        sim.register_node(node);

        // Try many times - with chaos config, should eventually crash
        let mut crashed = false;
        for i in 0..1000 {
            if sim.maybe_crash(&mut rng, node, VirtualTime(i)) {
                crashed = true;
                break;
            }
        }

        // With chaos config (0.5% crash rate), should very likely have crashed
        assert!(crashed, "Expected at least one crash with chaos config");
    }
}
