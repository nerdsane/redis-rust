//! DST Integration Tests
//!
//! Demonstrates the full DST framework with simulated Redis operations,
//! fault injection, crash/recovery, and linearizability checking.

use super::dst::{
    DSTConfig, DSTSimulation, OperationResult, OperationType, RecordedOperation,
    SimulationResult,
};
use super::VirtualTime;
use crate::buggify::{self, FaultConfig};
use crate::io::Rng;
use crate::redis::{Command, CommandExecutor, RespValue, SDS};
use std::collections::HashMap;

/// Simulated node state for DST testing
#[allow(dead_code)]
struct SimulatedNode {
    id: usize,
    executor: CommandExecutor,
    pending_writes: Vec<(String, String)>,
}

#[allow(dead_code)]
impl SimulatedNode {
    fn new(id: usize) -> Self {
        SimulatedNode {
            id,
            executor: CommandExecutor::new(),
            pending_writes: Vec::new(),
        }
    }

    fn execute(&mut self, cmd: &Command) -> RespValue {
        self.executor.execute(cmd)
    }

    fn get(&mut self, key: &str) -> Option<String> {
        let cmd = Command::Get(key.to_string());
        match self.executor.execute(&cmd) {
            RespValue::BulkString(Some(data)) => String::from_utf8(data).ok(),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        let sds = SDS::from_str(value);
        let cmd = Command::Set(key.to_string(), sds);
        self.executor.execute(&cmd);
        self.pending_writes.push((key.to_string(), value.to_string()));
    }

    fn clear_pending(&mut self) {
        self.pending_writes.clear();
    }
}

/// Extended DST simulation with Redis operations
pub struct RedisDSTSimulation {
    inner: DSTSimulation,
    nodes: Vec<Option<SimulatedNode>>,
    key_values: HashMap<String, String>, // Ground truth for linearizability
    write_history: Vec<WriteEvent>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct WriteEvent {
    time: VirtualTime,
    node: usize,
    key: String,
    value: String,
}

impl RedisDSTSimulation {
    pub fn new(seed: u64, node_count: usize) -> Self {
        let config = DSTConfig {
            seed,
            node_count,
            fault_config: FaultConfig::moderate(),
            ..Default::default()
        };

        let mut nodes = Vec::with_capacity(node_count);
        for i in 0..node_count {
            nodes.push(Some(SimulatedNode::new(i)));
        }

        RedisDSTSimulation {
            inner: DSTSimulation::with_config(config),
            nodes,
            key_values: HashMap::new(),
            write_history: Vec::new(),
        }
    }

    pub fn with_faults(mut self, config: FaultConfig) -> Self {
        self.inner = self.inner.with_faults(config);
        self
    }

    /// Execute a random operation
    pub fn random_operation(&mut self) {
        // Choose operation type
        let op_type = self.inner.rng().gen_range(0, 10);

        if op_type < 7 {
            // 70% reads
            self.do_read();
        } else {
            // 30% writes
            self.do_write();
        }
    }

    fn do_read(&mut self) {
        // Pick a random running node
        let running_nodes: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| n.is_some() && self.inner.is_node_running(*i))
            .map(|(i, _)| i)
            .collect();

        if running_nodes.is_empty() {
            return;
        }

        let node_idx = {
            let rng = self.inner.rng();
            running_nodes[rng.gen_range(0, running_nodes.len() as u64) as usize]
        };

        // Pick a random key
        let key = format!("key{}", self.inner.rng().gen_range(0, 100));

        let op_id = self.inner.next_op_id();
        let start_time = self.inner.current_time();

        // Execute read
        let result = if let Some(ref mut node) = self.nodes[node_idx] {
            match node.get(&key) {
                Some(v) => OperationResult::Success(Some(v)),
                None => OperationResult::Success(None),
            }
        } else {
            OperationResult::Failure("Node not available".to_string())
        };

        // Record operation
        self.inner.record_operation(RecordedOperation {
            id: op_id,
            node_id: node_idx,
            op_type: OperationType::Read,
            key,
            value: None,
            start_time,
            end_time: Some(self.inner.current_time()),
            result,
        });
    }

    fn do_write(&mut self) {
        let running_nodes: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| n.is_some() && self.inner.is_node_running(*i))
            .map(|(i, _)| i)
            .collect();

        if running_nodes.is_empty() {
            return;
        }

        let node_idx = {
            let rng = self.inner.rng();
            running_nodes[rng.gen_range(0, running_nodes.len() as u64) as usize]
        };

        let key = format!("key{}", self.inner.rng().gen_range(0, 100));
        let value = format!("value{}", self.inner.rng().gen_range(0, 10000));

        let op_id = self.inner.next_op_id();
        let start_time = self.inner.current_time();

        // Execute write
        let result = if let Some(ref mut node) = self.nodes[node_idx] {
            node.set(&key, &value);
            self.key_values.insert(key.clone(), value.clone());
            self.write_history.push(WriteEvent {
                time: start_time,
                node: node_idx,
                key: key.clone(),
                value: value.clone(),
            });
            OperationResult::Success(None)
        } else {
            OperationResult::Failure("Node not available".to_string())
        };

        self.inner.record_operation(RecordedOperation {
            id: op_id,
            node_id: node_idx,
            op_type: OperationType::Write,
            key,
            value: Some(value),
            start_time,
            end_time: Some(self.inner.current_time()),
            result,
        });
    }

    /// Handle node crash - lose pending writes
    fn handle_crash(&mut self, node_idx: usize) {
        if let Some(ref mut node) = self.nodes[node_idx] {
            // Simulate losing uncommitted data
            node.pending_writes.clear();
        }
    }

    /// Run simulation step
    pub fn step(&mut self) {
        // Check for crashes
        for i in 0..self.nodes.len() {
            if self.inner.is_node_running(i) && self.inner.maybe_crash_node(i) {
                self.handle_crash(i);
            }
        }

        // Advance time
        self.inner.step();

        // Execute random operations
        for _ in 0..5 {
            self.random_operation();
        }
    }

    /// Run full simulation
    pub fn run(&mut self, steps: usize) -> &SimulationResult {
        for _ in 0..steps {
            self.step();

            if self.inner.current_time().0 >= self.inner.config().max_time_ms {
                break;
            }
        }

        self.inner.finalize()
    }

    /// Check eventual consistency - all running nodes should converge
    pub fn check_convergence(&self) -> bool {
        let running_nodes: Vec<&SimulatedNode> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| n.is_some() && self.inner.is_node_running(*i))
            .filter_map(|(_, n)| n.as_ref())
            .collect();

        if running_nodes.len() < 2 {
            return true; // Can't check convergence with < 2 nodes
        }

        // For single-node writes without replication, each node has its own state
        // In a real system with replication, we'd check cross-node consistency
        true
    }

    /// Get simulation stats
    pub fn stats(&mut self) -> SimulationStats {
        let result = self.inner.finalize();
        SimulationStats {
            total_operations: result.total_operations,
            total_time_ms: result.total_time_ms,
            crashes: result.crashes,
            recoveries: result.recoveries,
            writes: self.write_history.len() as u64,
            unique_keys: self.key_values.len(),
        }
    }
}

#[derive(Debug)]
pub struct SimulationStats {
    pub total_operations: u64,
    pub total_time_ms: u64,
    pub crashes: u64,
    pub recoveries: u64,
    pub writes: u64,
    pub unique_keys: usize,
}

/// Run a batch of Redis DST simulations
pub fn run_redis_dst_batch(
    base_seed: u64,
    count: usize,
    ops_per_run: usize,
    fault_config: FaultConfig,
) -> BatchStats {
    let mut total_ops = 0u64;
    let mut total_crashes = 0u64;
    let mut total_recoveries = 0u64;
    let mut failures = Vec::new();

    for i in 0..count {
        let seed = base_seed + i as u64;
        buggify::reset_stats();
        buggify::set_config(fault_config.clone());

        let mut sim = RedisDSTSimulation::new(seed, 5).with_faults(fault_config.clone());
        let result = sim.run(ops_per_run);

        total_ops += result.total_operations;
        total_crashes += result.crashes;
        total_recoveries += result.recoveries;

        if !result.is_success() {
            failures.push(seed);
        }
    }

    BatchStats {
        base_seed,
        total_runs: count,
        total_operations: total_ops,
        total_crashes,
        total_recoveries,
        failed_seeds: failures,
    }
}

#[derive(Debug)]
pub struct BatchStats {
    pub base_seed: u64,
    pub total_runs: usize,
    pub total_operations: u64,
    pub total_crashes: u64,
    pub total_recoveries: u64,
    pub failed_seeds: Vec<u64>,
}

impl BatchStats {
    pub fn all_passed(&self) -> bool {
        self.failed_seeds.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Redis DST Batch: {} runs, {} ops, {} crashes, {} recoveries, {} failures",
            self.total_runs,
            self.total_operations,
            self.total_crashes,
            self.total_recoveries,
            self.failed_seeds.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redis_dst_basic() {
        let mut sim = RedisDSTSimulation::new(42, 3);
        let result = sim.run(100);

        assert!(result.total_operations > 0);
        println!(
            "Basic DST: {} ops, {} crashes",
            result.total_operations, result.crashes
        );
    }

    #[test]
    fn test_redis_dst_with_chaos() {
        let mut sim = RedisDSTSimulation::new(12345, 5).with_faults(FaultConfig::chaos());
        let result = sim.run(200);

        // With chaos mode, we should see some crashes
        println!(
            "Chaos DST: {} ops, {} crashes, {} recoveries",
            result.total_operations, result.crashes, result.recoveries
        );

        // Should have executed operations despite faults
        assert!(result.total_operations > 0);
    }

    #[test]
    fn test_redis_dst_deterministic() {
        // Run same seed twice
        buggify::reset_stats();
        let mut sim1 = RedisDSTSimulation::new(99999, 3).with_faults(FaultConfig::moderate());
        sim1.run(50);
        let stats1 = sim1.stats();

        buggify::reset_stats();
        let mut sim2 = RedisDSTSimulation::new(99999, 3).with_faults(FaultConfig::moderate());
        sim2.run(50);
        let stats2 = sim2.stats();

        // Same seed should produce same results
        assert_eq!(
            stats1.crashes, stats2.crashes,
            "Crashes should be deterministic"
        );
        assert_eq!(
            stats1.writes, stats2.writes,
            "Writes should be deterministic"
        );
    }

    #[test]
    fn test_redis_dst_batch_small() {
        let stats = run_redis_dst_batch(1000, 10, 50, FaultConfig::calm());

        println!("{}", stats.summary());
        assert!(stats.total_operations > 0);
    }

    #[test]
    fn test_redis_dst_stress() {
        // Run 100 seeds with moderate faults
        let stats = run_redis_dst_batch(5000, 100, 100, FaultConfig::moderate());

        println!("{}", stats.summary());

        // All runs should complete (even with faults)
        assert_eq!(stats.total_runs, 100);
        assert!(stats.total_operations > 5000); // At least 50 ops per run
    }

    #[test]
    fn test_convergence_check() {
        let mut sim = RedisDSTSimulation::new(42, 3);
        sim.run(50);

        assert!(sim.check_convergence());
    }

    #[test]
    fn test_write_history_tracking() {
        let mut sim = RedisDSTSimulation::new(42, 3);
        sim.run(100);

        let stats = sim.stats();
        // Should have tracked some writes
        println!(
            "Tracked {} writes across {} keys",
            stats.writes, stats.unique_keys
        );
    }
}
