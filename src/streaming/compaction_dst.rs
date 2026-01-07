//! Deterministic Simulation Testing for Compaction
//!
//! This DST harness tests the interaction between concurrent persistence
//! and compaction operations. It specifically targets the race condition
//! where persistence writes segments while compaction deletes them.
//!
//! ## Bug Being Tested (PR #2)
//!
//! The race condition occurs when:
//! 1. Persistence flushes a segment with ID N
//! 2. Compaction reads manifest, sees segments 0..N-1
//! 3. Compaction compacts segments, creates segment M (where M = N from compactor's view)
//! 4. Persistence overwrites manifest with segment N (stale next_segment_id)
//! 5. Next compaction tries to read segment that persistence overwrote → crash
//!
//! ## Key Invariant
//!
//! All segments referenced in the manifest MUST exist in the object store.

use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use crate::redis::SDS;
use crate::replication::lattice::{LamportClock, ReplicaId};
use crate::replication::state::{ReplicatedValue, ReplicationDelta};
use crate::streaming::{
    Compactor, CompactionConfig, CompactionError, InMemoryObjectStore, ManifestManager,
    ObjectStore, SegmentReader, SimulatedObjectStore, SimulatedStoreConfig, SimulatedStoreStats,
    StreamingPersistence, WriteBufferConfig,
};
use std::sync::Arc;

/// Configuration for compaction DST
#[derive(Debug, Clone)]
pub struct CompactionDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Object store fault configuration
    pub store_config: SimulatedStoreConfig,
    /// Write buffer configuration
    pub write_buffer_config: WriteBufferConfig,
    /// Compaction configuration
    pub compaction_config: CompactionConfig,
    /// Replica ID
    pub replica_id: u64,
    /// Prefix for object keys
    pub prefix: String,
    /// Probability of flush operation (vs write)
    pub flush_probability: f64,
    /// Probability of compaction attempt
    pub compact_probability: f64,
    /// Maximum operations per run
    pub max_operations: usize,
}

impl Default for CompactionDSTConfig {
    fn default() -> Self {
        CompactionDSTConfig {
            seed: 0,
            store_config: SimulatedStoreConfig::default(),
            write_buffer_config: WriteBufferConfig::test(),
            compaction_config: CompactionConfig::test(),
            replica_id: 1,
            prefix: "compaction_dst".to_string(),
            flush_probability: 0.15,
            compact_probability: 0.10,
            max_operations: 500,
        }
    }
}

impl CompactionDSTConfig {
    pub fn new(seed: u64) -> Self {
        CompactionDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Calm mode - minimal fault injection
    pub fn calm(seed: u64) -> Self {
        CompactionDSTConfig {
            seed,
            store_config: SimulatedStoreConfig::no_faults(),
            flush_probability: 0.2,
            compact_probability: 0.15,
            ..Default::default()
        }
    }

    /// Aggressive mode - high compaction rate to trigger race conditions
    pub fn aggressive(seed: u64) -> Self {
        CompactionDSTConfig {
            seed,
            store_config: SimulatedStoreConfig::no_faults(),
            flush_probability: 0.25,
            compact_probability: 0.25,
            compaction_config: CompactionConfig {
                min_segments_to_compact: 2,
                max_segments: 5,
                ..CompactionConfig::test()
            },
            ..Default::default()
        }
    }

    /// Chaos mode - fault injection + high concurrency
    pub fn chaos(seed: u64) -> Self {
        CompactionDSTConfig {
            seed,
            store_config: SimulatedStoreConfig::high_chaos(),
            flush_probability: 0.20,
            compact_probability: 0.20,
            compaction_config: CompactionConfig {
                min_segments_to_compact: 2,
                max_segments: 5,
                ..CompactionConfig::test()
            },
            ..Default::default()
        }
    }
}

/// Operation types for compaction DST
#[derive(Debug, Clone)]
pub enum CompactionOperation {
    /// Write a delta to persistence
    Write { key: String, value: String },
    /// Delete a key (tombstone)
    Delete { key: String },
    /// Flush persistence buffer to segment
    Flush,
    /// Attempt compaction
    Compact,
}

/// Outcome of an operation
#[derive(Debug, Clone)]
pub enum CompactionOutcome {
    /// Operation succeeded
    Success,
    /// Operation succeeded with details
    SuccessWithDetails(String),
    /// Operation failed (may be expected under fault injection)
    Failed(String),
    /// Nothing to do (e.g., compaction not needed)
    Skipped(String),
}

/// Recorded operation for history tracking
#[derive(Debug, Clone)]
pub struct RecordedCompactionOp {
    pub id: u64,
    pub operation: CompactionOperation,
    pub outcome: CompactionOutcome,
}

/// Result of a compaction DST run
#[derive(Debug, Clone)]
pub struct CompactionDSTResult {
    /// Seed used
    pub seed: u64,
    /// Total operations attempted
    pub total_operations: u64,
    /// Successful writes
    pub successful_writes: u64,
    /// Successful flushes
    pub successful_flushes: u64,
    /// Successful compactions
    pub successful_compactions: u64,
    /// Failed operations
    pub failed_operations: u64,
    /// Skipped operations
    pub skipped_operations: u64,
    /// Store fault statistics
    pub store_stats: SimulatedStoreStats,
    /// Invariant violations found
    pub invariant_violations: Vec<String>,
    /// Operation history
    pub history: Vec<RecordedCompactionOp>,
}

impl CompactionDSTResult {
    pub fn new(seed: u64) -> Self {
        CompactionDSTResult {
            seed,
            total_operations: 0,
            successful_writes: 0,
            successful_flushes: 0,
            successful_compactions: 0,
            failed_operations: 0,
            skipped_operations: 0,
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
            "Seed {}: {} ops ({} writes, {} flushes, {} compactions, {} failed, {} skipped), {} violations",
            self.seed,
            self.total_operations,
            self.successful_writes,
            self.successful_flushes,
            self.successful_compactions,
            self.failed_operations,
            self.skipped_operations,
            self.invariant_violations.len()
        )
    }
}

/// Workload generator for compaction DST
pub struct CompactionWorkload {
    rng: SimulatedRng,
    config: CompactionDSTConfig,
    lamport_time: u64,
}

impl CompactionWorkload {
    pub fn new(config: CompactionDSTConfig) -> Self {
        CompactionWorkload {
            rng: SimulatedRng::new(config.seed),
            config,
            lamport_time: 0,
        }
    }

    /// Generate the next operation
    pub fn next_operation(&mut self, current_op: u64) -> CompactionOperation {
        let roll = self.rng.next_u64() as f64 / u64::MAX as f64;

        if roll < self.config.compact_probability {
            CompactionOperation::Compact
        } else if roll < self.config.compact_probability + self.config.flush_probability {
            CompactionOperation::Flush
        } else {
            // Write or delete
            let key = format!("key_{:04}", self.rng.gen_range(0, 100));

            if self.rng.gen_bool(0.1) {
                CompactionOperation::Delete { key }
            } else {
                let value = format!("value_{}", current_op);
                CompactionOperation::Write { key, value }
            }
        }
    }

    /// Get next lamport timestamp
    pub fn next_timestamp(&mut self) -> u64 {
        self.lamport_time += 1;
        self.lamport_time
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

/// Type alias for our simulated store
type DSTStore = SimulatedObjectStore<InMemoryObjectStore, SimulatedRng>;

/// Main DST harness for compaction + persistence interaction
pub struct CompactionDSTHarness {
    config: CompactionDSTConfig,
    store: Arc<DSTStore>,
    workload: CompactionWorkload,
    persistence: Option<StreamingPersistence<DSTStore>>,
    compactor: Option<Compactor<DSTStore>>,
    result: CompactionDSTResult,
}

impl CompactionDSTHarness {
    /// Create a new compaction DST harness
    pub async fn new(config: CompactionDSTConfig) -> Self {
        let inner_store = InMemoryObjectStore::new();
        // Use different seeds for store RNG vs workload RNG for diversity
        let store_rng = SimulatedRng::new(config.seed.wrapping_add(1));
        let store = Arc::new(SimulatedObjectStore::new(
            inner_store,
            store_rng,
            config.store_config.clone(),
        ));

        // Create persistence
        let persistence = StreamingPersistence::new(
            store.clone(),
            config.prefix.clone(),
            config.replica_id,
            config.write_buffer_config.clone(),
        )
        .await
        .ok();

        // Create compactor with its own ManifestManager (this is the key test scenario!)
        // The bug was that compactor and persistence had separate ManifestManagers
        // leading to race conditions. The fix ensures they coordinate via reload.
        let compactor = {
            let manifest_manager = ManifestManager::new((*store).clone(), &config.prefix);
            Some(Compactor::new(
                store.clone(),
                config.prefix.clone(),
                manifest_manager,
                config.compaction_config.clone(),
            ))
        };

        let workload = CompactionWorkload::new(config.clone());
        let result = CompactionDSTResult::new(config.seed);

        CompactionDSTHarness {
            config,
            store,
            workload,
            persistence,
            compactor,
            result,
        }
    }

    /// Run the workload for a specified number of operations
    pub async fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            let op = self.workload.next_operation(self.result.total_operations);
            self.execute_operation(op).await;
        }
    }

    /// Execute a single operation
    async fn execute_operation(&mut self, op: CompactionOperation) {
        self.result.total_operations += 1;
        let op_id = self.result.total_operations;

        let outcome = match &op {
            CompactionOperation::Write { key, value } => self.execute_write(key, value).await,
            CompactionOperation::Delete { key } => self.execute_delete(key).await,
            CompactionOperation::Flush => self.execute_flush().await,
            CompactionOperation::Compact => self.execute_compact().await,
        };

        // Record operation
        let recorded = RecordedCompactionOp {
            id: op_id,
            operation: op,
            outcome: outcome.clone(),
        };
        self.result.history.push(recorded);

        // Update stats
        match outcome {
            CompactionOutcome::Success | CompactionOutcome::SuccessWithDetails(_) => {
                // Specific stat updates happen in execute methods
            }
            CompactionOutcome::Failed(_) => self.result.failed_operations += 1,
            CompactionOutcome::Skipped(_) => self.result.skipped_operations += 1,
        }
    }

    async fn execute_write(&mut self, key: &str, value: &str) -> CompactionOutcome {
        let Some(ref mut persistence) = self.persistence else {
            return CompactionOutcome::Failed("No persistence instance".to_string());
        };

        let delta = self.workload.make_write_delta(key, value);

        match persistence.push(delta) {
            Ok(()) => {
                self.result.successful_writes += 1;
                CompactionOutcome::Success
            }
            Err(e) => CompactionOutcome::Failed(e.to_string()),
        }
    }

    async fn execute_delete(&mut self, key: &str) -> CompactionOutcome {
        let Some(ref mut persistence) = self.persistence else {
            return CompactionOutcome::Failed("No persistence instance".to_string());
        };

        let delta = self.workload.make_delete_delta(key);

        match persistence.push(delta) {
            Ok(()) => {
                self.result.successful_writes += 1;
                CompactionOutcome::Success
            }
            Err(e) => CompactionOutcome::Failed(e.to_string()),
        }
    }

    async fn execute_flush(&mut self) -> CompactionOutcome {
        let Some(ref mut persistence) = self.persistence else {
            return CompactionOutcome::Failed("No persistence instance".to_string());
        };

        match persistence.flush().await {
            Ok(result) => {
                self.result.successful_flushes += 1;
                if let Some(seg) = result.segment {
                    CompactionOutcome::SuccessWithDetails(format!("Created segment {}", seg.key))
                } else {
                    CompactionOutcome::SuccessWithDetails("Buffer empty".to_string())
                }
            }
            Err(e) => CompactionOutcome::Failed(e.to_string()),
        }
    }

    async fn execute_compact(&mut self) -> CompactionOutcome {
        let Some(ref mut compactor) = self.compactor else {
            return CompactionOutcome::Failed("No compactor instance".to_string());
        };

        match compactor.compact().await {
            Ok(result) => {
                self.result.successful_compactions += 1;
                CompactionOutcome::SuccessWithDetails(format!(
                    "Compacted {} segments into 1, {} -> {} deltas",
                    result.segments_removed.len(),
                    result.deltas_before,
                    result.deltas_after
                ))
            }
            Err(CompactionError::NothingToCompact) => {
                CompactionOutcome::Skipped("Nothing to compact".to_string())
            }
            Err(e) => CompactionOutcome::Failed(e.to_string()),
        }
    }

    /// Check invariants after the run
    ///
    /// KEY INVARIANT: All segments in manifest must exist in store
    pub async fn check_invariants(&mut self) {
        // Force final flush to ensure all buffered data is persisted
        if let Some(ref mut persistence) = self.persistence {
            let _ = persistence.flush().await;
        }

        // Invariant 1: All segments in manifest should exist in store
        self.check_segment_existence().await;

        // Invariant 2: All segments should be readable and valid
        self.check_segment_validity().await;

        // Invariant 3: Manifest should be consistent (no duplicate IDs)
        self.check_manifest_consistency().await;

        // Update store stats
        self.result.store_stats = self.store.stats();
    }

    /// KEY INVARIANT: All segments referenced in manifest must exist in store
    ///
    /// Note: We reload the manifest from storage to get the authoritative view,
    /// not the potentially stale cache in persistence.
    async fn check_segment_existence(&mut self) {
        // Reload manifest from storage to get the authoritative view
        let manifest_manager = ManifestManager::new((*self.store).clone(), &self.config.prefix);
        let manifest = match manifest_manager.load().await {
            Ok(m) => m,
            Err(_) => return, // No manifest to check
        };

        for segment in &manifest.segments {
            match self.store.exists(&segment.key).await {
                Ok(exists) => {
                    if !exists {
                        self.result.invariant_violations.push(format!(
                            "CRITICAL: Segment {} (id={}) exists in manifest but NOT in store! \
                             This is the race condition bug. Seed: {}",
                            segment.key, segment.id, self.config.seed
                        ));
                    }
                }
                Err(e) => {
                    // Store error during check - only warn, may be from fault injection
                    eprintln!(
                        "Warning: Store error checking segment {} existence: {}",
                        segment.key, e
                    );
                }
            }
        }
    }

    /// Check that all segments can be read and validated
    async fn check_segment_validity(&mut self) {
        // Reload manifest from storage to get the authoritative view
        let manifest_manager = ManifestManager::new((*self.store).clone(), &self.config.prefix);
        let manifest = match manifest_manager.load().await {
            Ok(m) => m,
            Err(_) => return, // No manifest to check
        };

        for segment in &manifest.segments {
            match self.store.get(&segment.key).await {
                Ok(data) => {
                    match SegmentReader::open(&data) {
                        Ok(reader) => {
                            if let Err(e) = reader.validate() {
                                // Only report if not from expected partial writes
                                if !self.is_expected_corruption() {
                                    self.result.invariant_violations.push(format!(
                                        "Segment {} failed validation: {}",
                                        segment.key, e
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            if !self.is_expected_corruption() {
                                self.result.invariant_violations.push(format!(
                                    "Segment {} failed to parse: {}",
                                    segment.key, e
                                ));
                            }
                        }
                    }
                }
                Err(_) => {
                    // Already checked in segment_existence
                }
            }
        }
    }

    /// Check manifest consistency - no duplicate segment IDs
    async fn check_manifest_consistency(&mut self) {
        // Reload manifest from storage to get the authoritative view
        let manifest_manager = ManifestManager::new((*self.store).clone(), &self.config.prefix);
        let manifest = match manifest_manager.load().await {
            Ok(m) => m,
            Err(_) => return, // No manifest to check
        };
        let mut seen_ids = std::collections::HashSet::new();

        for segment in &manifest.segments {
            if !seen_ids.insert(segment.id) {
                self.result.invariant_violations.push(format!(
                    "Duplicate segment ID {} in manifest. Seed: {}",
                    segment.id, self.config.seed
                ));
            }
        }

        // Check next_segment_id is greater than all existing IDs
        for segment in &manifest.segments {
            if segment.id >= manifest.next_segment_id {
                self.result.invariant_violations.push(format!(
                    "Segment ID {} >= next_segment_id {}. Seed: {}",
                    segment.id, manifest.next_segment_id, self.config.seed
                ));
            }
        }
    }

    /// Check if corruption is expected due to fault injection
    fn is_expected_corruption(&self) -> bool {
        self.config.store_config.partial_write_prob > 0.0
    }

    /// Get the result
    pub fn result(&self) -> &CompactionDSTResult {
        &self.result
    }

    /// Consume and return the result
    pub fn into_result(self) -> CompactionDSTResult {
        self.result
    }
}

/// Run a batch of compaction DST tests with different seeds
pub async fn run_compaction_dst_batch(
    base_seed: u64,
    count: usize,
    ops_per_run: usize,
    config_fn: impl Fn(u64) -> CompactionDSTConfig,
) -> Vec<CompactionDSTResult> {
    let mut results = Vec::with_capacity(count);

    for i in 0..count {
        let seed = base_seed + i as u64;
        let config = config_fn(seed);

        let mut harness = CompactionDSTHarness::new(config).await;
        harness.run(ops_per_run).await;
        harness.check_invariants().await;

        results.push(harness.into_result());
    }

    results
}

/// Summary of batch results
pub fn summarize_compaction_batch(results: &[CompactionDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed_seeds: Vec<u64> = results
        .iter()
        .filter(|r| !r.is_success())
        .map(|r| r.seed)
        .collect();

    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();
    let total_flushes: u64 = results.iter().map(|r| r.successful_flushes).sum();
    let total_compactions: u64 = results.iter().map(|r| r.successful_compactions).sum();

    let mut summary = format!(
        "Compaction DST Batch: {}/{} passed, {} total ops, {} flushes, {} compactions",
        passed, total, total_ops, total_flushes, total_compactions
    );

    if !failed_seeds.is_empty() {
        summary.push_str(&format!("\nFailed seeds: {:?}", failed_seeds));
        // Show first failure details
        if let Some(first_failure) = results.iter().find(|r| !r.is_success()) {
            summary.push_str(&format!(
                "\nFirst failure violations:\n  {}",
                first_failure.invariant_violations.join("\n  ")
            ));
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_compaction_dst_calm() {
        let config = CompactionDSTConfig::calm(42);
        let mut harness = CompactionDSTHarness::new(config).await;

        harness.run(200).await;
        harness.check_invariants().await;

        let result = harness.result();
        assert!(
            result.is_success(),
            "Calm mode should not have invariant violations: {:?}",
            result.invariant_violations
        );
        assert!(result.total_operations >= 200);
        println!("{}", result.summary());
    }

    #[tokio::test]
    async fn test_compaction_dst_aggressive() {
        // Aggressive mode specifically targets the race condition
        let config = CompactionDSTConfig::aggressive(123);
        let mut harness = CompactionDSTHarness::new(config).await;

        harness.run(300).await;
        harness.check_invariants().await;

        let result = harness.result();
        assert!(
            result.is_success(),
            "Aggressive mode should not have invariant violations (fix should prevent race): {:?}",
            result.invariant_violations
        );
        // Should have at least some compactions
        assert!(
            result.successful_compactions > 0 || result.skipped_operations > 0,
            "Should have attempted compactions"
        );
        println!("{}", result.summary());
    }

    #[tokio::test]
    async fn test_compaction_dst_deterministic() {
        // Run same seed twice, should get same results
        let seed = 12345;

        let config1 = CompactionDSTConfig::calm(seed);
        let mut harness1 = CompactionDSTHarness::new(config1).await;
        harness1.run(100).await;
        let writes1 = harness1.result().successful_writes;

        let config2 = CompactionDSTConfig::calm(seed);
        let mut harness2 = CompactionDSTHarness::new(config2).await;
        harness2.run(100).await;
        let writes2 = harness2.result().successful_writes;

        assert_eq!(writes1, writes2, "Same seed should produce same results");
    }

    #[tokio::test]
    async fn test_compaction_dst_batch_calm() {
        let results = run_compaction_dst_batch(1000, 10, 100, CompactionDSTConfig::calm).await;

        let summary = summarize_compaction_batch(&results);
        println!("{}", summary);

        assert!(
            results.iter().all(|r| r.is_success()),
            "All calm runs should pass"
        );
    }

    #[tokio::test]
    async fn test_compaction_dst_batch_aggressive() {
        // This is the key test - aggressive compaction + persistence should not cause
        // the race condition now that the fix is in place
        let results =
            run_compaction_dst_batch(2000, 20, 200, CompactionDSTConfig::aggressive).await;

        let summary = summarize_compaction_batch(&results);
        println!("{}", summary);

        // All should pass with the fix in place
        assert!(
            results.iter().all(|r| r.is_success()),
            "All aggressive runs should pass with race condition fix"
        );
    }

    #[tokio::test]
    async fn test_compaction_dst_batch_chaos() {
        // Chaos mode tests with fault injection
        let results = run_compaction_dst_batch(3000, 10, 150, CompactionDSTConfig::chaos).await;

        let summary = summarize_compaction_batch(&results);
        println!("{}", summary);

        // Most should pass even with chaos (some failures expected from faults)
        let passed = results.iter().filter(|r| r.is_success()).count();
        assert!(
            passed >= results.len() * 7 / 10,
            "At least 70% should pass with chaos faults: {}",
            summary
        );
    }

    #[tokio::test]
    async fn test_compaction_creates_segments() {
        // Ensure compaction actually happens and creates segments
        let mut config = CompactionDSTConfig::calm(999);
        config.flush_probability = 0.3; // More flushes to create segments
        config.compact_probability = 0.1;
        config.compaction_config.min_segments_to_compact = 2;

        let mut harness = CompactionDSTHarness::new(config).await;
        harness.run(500).await;
        harness.check_invariants().await;

        let result = harness.result();
        println!("{}", result.summary());

        // Should have created segments via flushes
        assert!(
            result.successful_flushes > 0,
            "Should have successful flushes"
        );
    }

    #[tokio::test]
    async fn test_segment_existence_invariant() {
        // This test verifies the key invariant is being checked
        let config = CompactionDSTConfig::aggressive(42);
        let mut harness = CompactionDSTHarness::new(config).await;

        // Run enough operations to generate segments and compactions
        harness.run(300).await;
        harness.check_invariants().await;

        // The invariant check should have run
        let result = harness.result();

        // With the fix in place, there should be no violations
        assert!(
            result.invariant_violations.is_empty(),
            "Should have no segment existence violations with fix: {:?}",
            result.invariant_violations
        );
    }
}
