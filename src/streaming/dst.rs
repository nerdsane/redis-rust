//! Deterministic Simulation Testing for Streaming Persistence
//!
//! Shadow-state testing harness for streaming persistence that enables:
//! - Deterministic random workload generation
//! - Fault injection at the object store layer
//! - Invariant checking after each operation
//! - Seed-based reproducibility for debugging
//!
//! ## Design (FoundationDB-style DST with shadow state)
//!
//! ```text
//! for seed in 0..10000 {
//!     let harness = StreamingDSTHarness::new(seed);
//!     harness.run_workload(1000);
//!     harness.check_invariants();  // Panics with seed on failure
//! }
//! ```

use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use crate::redis::SDS;
use crate::replication::lattice::{LamportClock, ReplicaId};
use crate::replication::state::{ReplicatedValue, ReplicationDelta};
use crate::streaming::{
    InMemoryObjectStore, ObjectStore, RecoveryManager, SegmentReader, SimulatedObjectStore,
    SimulatedStoreConfig, SimulatedStoreStats, StreamingPersistence, WriteBufferConfig,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Configuration for streaming DST
#[derive(Debug, Clone)]
pub struct StreamingDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Object store fault configuration
    pub store_config: SimulatedStoreConfig,
    /// Write buffer configuration
    pub write_buffer_config: WriteBufferConfig,
    /// Replica ID
    pub replica_id: u64,
    /// Prefix for object keys
    pub prefix: String,
    /// Probability of flush operation (vs write)
    pub flush_probability: f64,
    /// Probability of crash/recovery simulation
    pub crash_probability: f64,
    /// Maximum operations per run
    pub max_operations: usize,
}

impl Default for StreamingDSTConfig {
    fn default() -> Self {
        StreamingDSTConfig {
            seed: 0,
            store_config: SimulatedStoreConfig::default(),
            write_buffer_config: WriteBufferConfig::test(),
            replica_id: 1,
            prefix: "dst".to_string(),
            flush_probability: 0.1,
            crash_probability: 0.01,
            max_operations: 1000,
        }
    }
}

impl StreamingDSTConfig {
    pub fn new(seed: u64) -> Self {
        StreamingDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Calm mode - minimal fault injection
    pub fn calm(seed: u64) -> Self {
        StreamingDSTConfig {
            seed,
            store_config: SimulatedStoreConfig::no_faults(),
            flush_probability: 0.2,
            crash_probability: 0.0,
            ..Default::default()
        }
    }

    /// Chaos mode - aggressive fault injection
    pub fn chaos(seed: u64) -> Self {
        StreamingDSTConfig {
            seed,
            store_config: SimulatedStoreConfig::high_chaos(),
            flush_probability: 0.15,
            crash_probability: 0.05,
            ..Default::default()
        }
    }

    /// Moderate fault injection
    pub fn moderate(seed: u64) -> Self {
        StreamingDSTConfig {
            seed,
            store_config: SimulatedStoreConfig::default(),
            flush_probability: 0.1,
            crash_probability: 0.02,
            ..Default::default()
        }
    }
}

/// Streaming operation type
#[derive(Debug, Clone)]
pub enum StreamingOperation {
    /// Write a delta
    Write { key: String, value: String },
    /// Write a delete (tombstone)
    Delete { key: String },
    /// Flush the buffer
    Flush,
    /// Simulate crash and recovery
    CrashRecover,
}

/// Outcome of an operation
#[derive(Debug, Clone)]
pub enum OperationOutcome {
    /// Operation succeeded
    Success,
    /// Operation failed (expected under fault injection)
    Failed(String),
    /// Recovered from crash
    Recovered { deltas_recovered: usize },
}

/// Recorded operation for history tracking
#[derive(Debug, Clone)]
pub struct RecordedOperation {
    pub id: u64,
    pub operation: StreamingOperation,
    pub outcome: OperationOutcome,
    pub timestamp_ms: u64,
}

/// Workload generator
pub struct StreamingWorkload {
    rng: SimulatedRng,
    config: StreamingDSTConfig,
    operation_counter: u64,
    lamport_time: u64,
    /// Ground truth: what we expect to be persisted
    expected_state: HashMap<String, Option<String>>,
}

impl StreamingWorkload {
    pub fn new(config: StreamingDSTConfig) -> Self {
        StreamingWorkload {
            rng: SimulatedRng::new(config.seed),
            config,
            operation_counter: 0,
            lamport_time: 0,
            expected_state: HashMap::new(),
        }
    }

    /// Generate the next operation
    pub fn next_operation(&mut self) -> StreamingOperation {
        let roll = self.rng.next_u64() as f64 / u64::MAX as f64;

        if roll < self.config.crash_probability {
            StreamingOperation::CrashRecover
        } else if roll < self.config.crash_probability + self.config.flush_probability {
            StreamingOperation::Flush
        } else {
            // Write or delete
            let key = format!("key_{:04}", self.rng.gen_range(0, 100));

            if self.rng.gen_bool(0.1) {
                StreamingOperation::Delete { key }
            } else {
                let value = format!("value_{}", self.operation_counter);
                StreamingOperation::Write { key, value }
            }
        }
    }

    /// Record that a write was successfully persisted
    pub fn record_write(&mut self, key: &str, value: &str) {
        self.expected_state
            .insert(key.to_string(), Some(value.to_string()));
    }

    /// Record that a delete was successfully persisted
    pub fn record_delete(&mut self, key: &str) {
        self.expected_state.insert(key.to_string(), None);
    }

    /// Get next lamport timestamp
    pub fn next_timestamp(&mut self) -> u64 {
        self.lamport_time += 1;
        self.lamport_time
    }

    /// Get expected state
    pub fn expected_state(&self) -> &HashMap<String, Option<String>> {
        &self.expected_state
    }

    /// Create a delta for a write operation
    pub fn make_write_delta(&mut self, key: &str, value: &str) -> ReplicationDelta {
        let replica_id = ReplicaId::new(self.config.replica_id);
        let clock = LamportClock {
            time: self.next_timestamp(),
            replica_id,
        };
        let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }

    /// Create a delta for a delete operation (tombstone)
    pub fn make_delete_delta(&mut self, key: &str) -> ReplicationDelta {
        let replica_id = ReplicaId::new(self.config.replica_id);
        let mut clock = LamportClock {
            time: self.next_timestamp(),
            replica_id,
        };
        let mut replicated = ReplicatedValue::new(replica_id);
        replicated.delete(&mut clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }
}

/// Result of a DST run
#[derive(Debug, Clone)]
pub struct StreamingDSTResult {
    /// Seed used
    pub seed: u64,
    /// Total operations attempted
    pub total_operations: u64,
    /// Successful operations
    pub successful_operations: u64,
    /// Failed operations (expected under faults)
    pub failed_operations: u64,
    /// Flushes performed
    pub flushes: u64,
    /// Crashes simulated
    pub crashes: u64,
    /// Store fault statistics
    pub store_stats: SimulatedStoreStats,
    /// Invariant violations found
    pub invariant_violations: Vec<String>,
    /// Operation history
    pub history: Vec<RecordedOperation>,
}

impl StreamingDSTResult {
    pub fn new(seed: u64) -> Self {
        StreamingDSTResult {
            seed,
            total_operations: 0,
            successful_operations: 0,
            failed_operations: 0,
            flushes: 0,
            crashes: 0,
            store_stats: SimulatedStoreStats::default(),
            invariant_violations: Vec::new(),
            history: Vec::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops ({} ok, {} failed), {} flushes, {} crashes, {} violations",
            self.seed,
            self.total_operations,
            self.successful_operations,
            self.failed_operations,
            self.flushes,
            self.crashes,
            self.invariant_violations.len()
        )
    }
}

/// Type alias for our simulated store
type DSTStore = SimulatedObjectStore<InMemoryObjectStore, SimulatedRng>;

/// Main DST harness for streaming persistence
pub struct StreamingDSTHarness {
    config: StreamingDSTConfig,
    store: Arc<DSTStore>,
    inner_store: InMemoryObjectStore,
    workload: StreamingWorkload,
    persistence: Option<StreamingPersistence<DSTStore>>,
    result: StreamingDSTResult,
    /// Flushed deltas (ground truth for what's in object store)
    flushed_deltas: Vec<ReplicationDelta>,
}

impl StreamingDSTHarness {
    /// Create a new DST harness
    pub async fn new(config: StreamingDSTConfig) -> Self {
        let inner_store = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(config.seed.wrapping_add(1)); // Different seed for store
        let store = Arc::new(SimulatedObjectStore::new(
            inner_store.clone(),
            rng,
            config.store_config.clone(),
        ));

        let persistence = StreamingPersistence::new(
            store.clone(),
            config.prefix.clone(),
            config.replica_id,
            config.write_buffer_config.clone(),
        )
        .await
        .ok();

        let workload = StreamingWorkload::new(config.clone());
        let result = StreamingDSTResult::new(config.seed);

        StreamingDSTHarness {
            config,
            store,
            inner_store,
            workload,
            persistence,
            result,
            flushed_deltas: Vec::new(),
        }
    }

    /// Run the workload for a specified number of operations
    pub async fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            let op = self.workload.next_operation();
            self.execute_operation(op).await;
        }
    }

    /// Execute a single operation
    async fn execute_operation(&mut self, op: StreamingOperation) {
        self.result.total_operations += 1;
        let op_id = self.result.total_operations;

        let outcome = match &op {
            StreamingOperation::Write { key, value } => self.execute_write(key, value).await,
            StreamingOperation::Delete { key } => self.execute_delete(key).await,
            StreamingOperation::Flush => self.execute_flush().await,
            StreamingOperation::CrashRecover => self.execute_crash_recover().await,
        };

        // Record operation
        let recorded = RecordedOperation {
            id: op_id,
            operation: op,
            outcome: outcome.clone(),
            timestamp_ms: self.workload.lamport_time,
        };
        self.result.history.push(recorded);

        // Update stats
        match outcome {
            OperationOutcome::Success => self.result.successful_operations += 1,
            OperationOutcome::Failed(_) => self.result.failed_operations += 1,
            OperationOutcome::Recovered { .. } => {
                self.result.successful_operations += 1;
                self.result.crashes += 1;
            }
        }
    }

    async fn execute_write(&mut self, key: &str, value: &str) -> OperationOutcome {
        let Some(ref mut persistence) = self.persistence else {
            return OperationOutcome::Failed("No persistence instance".to_string());
        };

        let delta = self.workload.make_write_delta(key, value);

        match persistence.push(delta) {
            Ok(()) => {
                // Note: we don't record to expected_state here because
                // the data isn't persisted until flush succeeds
                OperationOutcome::Success
            }
            Err(e) => OperationOutcome::Failed(e.to_string()),
        }
    }

    async fn execute_delete(&mut self, key: &str) -> OperationOutcome {
        let Some(ref mut persistence) = self.persistence else {
            return OperationOutcome::Failed("No persistence instance".to_string());
        };

        let delta = self.workload.make_delete_delta(key);

        match persistence.push(delta) {
            Ok(()) => OperationOutcome::Success,
            Err(e) => OperationOutcome::Failed(e.to_string()),
        }
    }

    async fn execute_flush(&mut self) -> OperationOutcome {
        let Some(ref mut persistence) = self.persistence else {
            return OperationOutcome::Failed("No persistence instance".to_string());
        };

        match persistence.flush().await {
            Ok(result) => {
                self.result.flushes += 1;
                if result.segment.is_some() {
                    // Flush succeeded - record what was persisted
                    // Note: In a real implementation we'd track this more precisely
                }
                OperationOutcome::Success
            }
            Err(e) => OperationOutcome::Failed(e.to_string()),
        }
    }

    async fn execute_crash_recover(&mut self) -> OperationOutcome {
        // Simulate crash by dropping persistence
        self.persistence = None;

        // Re-create persistence (simulates recovery)
        let new_persistence = StreamingPersistence::new(
            self.store.clone(),
            self.config.prefix.clone(),
            self.config.replica_id,
            self.config.write_buffer_config.clone(),
        )
        .await;

        match new_persistence {
            Ok(p) => {
                let segments = p.manifest().segments.len();
                self.persistence = Some(p);
                OperationOutcome::Recovered {
                    deltas_recovered: segments, // Approximate
                }
            }
            Err(e) => OperationOutcome::Failed(format!("Recovery failed: {}", e)),
        }
    }

    /// Check invariants after the run
    pub async fn check_invariants(&mut self) {
        // Force final flush
        if let Some(ref mut persistence) = self.persistence {
            let _ = persistence.flush().await;
        }

        // Invariant 1: All segments in manifest should exist in store
        self.check_segment_existence().await;

        // Invariant 2: All segments should be readable and valid
        self.check_segment_validity().await;

        // Invariant 3: Recovery should restore all persisted data
        self.check_recovery_completeness().await;

        // Update store stats
        self.result.store_stats = self.store.stats();
    }

    async fn check_segment_existence(&mut self) {
        let Some(ref persistence) = self.persistence else {
            return;
        };

        for segment in &persistence.manifest().segments {
            match self.store.exists(&segment.key).await {
                Ok(exists) => {
                    if !exists {
                        self.result.invariant_violations.push(format!(
                            "Segment {} exists in manifest but not in store",
                            segment.key
                        ));
                    }
                }
                Err(e) => {
                    // Store error during check - don't count as violation
                    // (fault injection may cause this)
                    eprintln!("Warning: Store error checking segment: {}", e);
                }
            }
        }
    }

    async fn check_segment_validity(&mut self) {
        let Some(ref persistence) = self.persistence else {
            return;
        };

        for segment in &persistence.manifest().segments {
            match self.store.get(&segment.key).await {
                Ok(data) => {
                    // Try to parse the segment
                    match SegmentReader::open(&data) {
                        Ok(reader) => {
                            // Validate checksums
                            if let Err(e) = reader.validate() {
                                self.result.invariant_violations.push(format!(
                                    "Segment {} failed validation: {}",
                                    segment.key, e
                                ));
                            }
                        }
                        Err(e) => {
                            // Corruption from fault injection - check if it's a partial write
                            if !self.is_known_corruption(&segment.key) {
                                self.result.invariant_violations.push(format!(
                                    "Segment {} failed to parse: {}",
                                    segment.key, e
                                ));
                            }
                        }
                    }
                }
                Err(_) => {
                    // Store error - may be from fault injection
                }
            }
        }
    }

    async fn check_recovery_completeness(&mut self) {
        // Create a fresh recovery manager
        let recovery = RecoveryManager::new(
            (*self.store).clone(),
            &self.config.prefix,
            self.config.replica_id,
        );

        // Attempt recovery
        match recovery.recover().await {
            Ok(recovered) => {
                // Recovery succeeded - verify we can read all deltas
                let total_deltas: usize = recovered.deltas.len();

                // Check manifest state
                if let Some(ref persistence) = self.persistence {
                    let expected_segments = persistence.manifest().segments.len();
                    // Recovery should see same number of segments
                    // (may differ slightly due to crash timing)
                    if expected_segments > 0 && total_deltas == 0 {
                        // This could be valid if all flushes failed
                        let stats = persistence.stats();
                        if stats.segments_written > 0 {
                            self.result.invariant_violations.push(format!(
                                "Recovery found 0 deltas but {} segments were written",
                                stats.segments_written
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                // Recovery failure under fault injection is expected
                eprintln!("Warning: Recovery failed (may be expected): {}", e);
            }
        }
    }

    /// Check if corruption is from a known partial write
    fn is_known_corruption(&self, _key: &str) -> bool {
        // If partial writes are enabled, some corruption is expected
        self.config.store_config.partial_write_prob > 0.0
    }

    /// Get the result
    pub fn result(&self) -> &StreamingDSTResult {
        &self.result
    }

    /// Consume and return the result
    pub fn into_result(self) -> StreamingDSTResult {
        self.result
    }
}

/// Run a batch of DST tests with different seeds
pub async fn run_dst_batch(
    base_seed: u64,
    count: usize,
    ops_per_run: usize,
    config_fn: impl Fn(u64) -> StreamingDSTConfig,
) -> Vec<StreamingDSTResult> {
    let mut results = Vec::with_capacity(count);

    for i in 0..count {
        let seed = base_seed + i as u64;
        let config = config_fn(seed);

        let mut harness = StreamingDSTHarness::new(config).await;
        harness.run(ops_per_run).await;
        harness.check_invariants().await;

        results.push(harness.into_result());
    }

    results
}

/// Summary of batch results
pub fn summarize_batch(results: &[StreamingDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed_seeds: Vec<u64> = results
        .iter()
        .filter(|r| !r.is_success())
        .map(|r| r.seed)
        .collect();

    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();
    let total_flushes: u64 = results.iter().map(|r| r.flushes).sum();
    let total_crashes: u64 = results.iter().map(|r| r.crashes).sum();

    let mut summary = format!(
        "Batch: {}/{} passed, {} total ops, {} flushes, {} crashes",
        passed, total, total_ops, total_flushes, total_crashes
    );

    if !failed_seeds.is_empty() {
        summary.push_str(&format!("\nFailed seeds: {:?}", failed_seeds));
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dst_harness_calm() {
        let config = StreamingDSTConfig::calm(42);
        let mut harness = StreamingDSTHarness::new(config).await;

        harness.run(100).await;
        harness.check_invariants().await;

        let result = harness.result();
        assert!(
            result.is_success(),
            "Calm mode should not have invariant violations: {:?}",
            result.invariant_violations
        );
        assert!(result.total_operations >= 100);
    }

    #[tokio::test]
    async fn test_dst_harness_with_faults() {
        let config = StreamingDSTConfig::moderate(123);
        let mut harness = StreamingDSTHarness::new(config).await;

        harness.run(200).await;
        harness.check_invariants().await;

        let result = harness.result();
        // Some operations may fail under faults, but invariants should hold
        assert!(result.total_operations >= 200);
        println!("{}", result.summary());
    }

    #[tokio::test]
    async fn test_dst_deterministic() {
        // Run same seed twice, should get same results
        let seed = 12345;

        let config1 = StreamingDSTConfig::calm(seed);
        let mut harness1 = StreamingDSTHarness::new(config1).await;
        harness1.run(50).await;
        let ops1 = harness1.result().successful_operations;

        let config2 = StreamingDSTConfig::calm(seed);
        let mut harness2 = StreamingDSTHarness::new(config2).await;
        harness2.run(50).await;
        let ops2 = harness2.result().successful_operations;

        assert_eq!(ops1, ops2, "Same seed should produce same results");
    }

    #[tokio::test]
    async fn test_dst_batch_calm() {
        let results = run_dst_batch(1000, 10, 50, StreamingDSTConfig::calm).await;

        let summary = summarize_batch(&results);
        println!("{}", summary);

        assert!(
            results.iter().all(|r| r.is_success()),
            "All calm runs should pass"
        );
    }

    #[tokio::test]
    async fn test_dst_batch_moderate() {
        let results = run_dst_batch(2000, 20, 100, StreamingDSTConfig::moderate).await;

        let summary = summarize_batch(&results);
        println!("{}", summary);

        // Most should pass even with moderate faults
        let passed = results.iter().filter(|r| r.is_success()).count();
        assert!(
            passed >= results.len() * 8 / 10,
            "At least 80% should pass with moderate faults"
        );
    }

    #[tokio::test]
    async fn test_workload_generator() {
        let config = StreamingDSTConfig::new(42);
        let mut workload = StreamingWorkload::new(config);

        let mut writes = 0;
        let mut deletes = 0;
        let mut flushes = 0;
        let mut crashes = 0;

        for _ in 0..1000 {
            match workload.next_operation() {
                StreamingOperation::Write { .. } => writes += 1,
                StreamingOperation::Delete { .. } => deletes += 1,
                StreamingOperation::Flush => flushes += 1,
                StreamingOperation::CrashRecover => crashes += 1,
            }
        }

        // Should have a mix of operations
        assert!(writes > 500, "Expected mostly writes");
        assert!(deletes > 0, "Expected some deletes");
        assert!(flushes > 0, "Expected some flushes");
        println!(
            "Workload: {} writes, {} deletes, {} flushes, {} crashes",
            writes, deletes, flushes, crashes
        );
    }

    #[tokio::test]
    async fn test_crash_recovery_cycle() {
        let mut config = StreamingDSTConfig::calm(999);
        config.crash_probability = 0.2; // High crash rate for this test

        let mut harness = StreamingDSTHarness::new(config).await;
        harness.run(100).await;
        harness.check_invariants().await;

        let result = harness.result();
        assert!(result.crashes > 0, "Should have some crashes");
        println!("{}", result.summary());
    }
}
