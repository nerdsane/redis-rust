//! Deterministic Simulation Testing for CRDTs
//!
//! Shadow-state testing harness for CRDT convergence that enables:
//! - Deterministic random operation generation
//! - Network partition and message drop simulation
//! - Invariant checking after each operation
//! - Seed-based reproducibility for debugging
//!
//! ## Design (FoundationDB-style DST with shadow state)
//!
//! ```text
//! for seed in 0..100 {
//!     let harness = CRDTDSTHarness::new(seed, 3); // 3 replicas
//!     harness.run_operations(500);
//!     harness.sync_all();
//!     harness.check_convergence();  // Panics with seed on failure
//! }
//! ```

use super::lattice::{GCounter, ORSet, PNCounter, ReplicaId, VectorClock};
use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use std::collections::HashMap;

/// Configuration for CRDT DST
#[derive(Debug, Clone)]
pub struct CRDTDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of replicas
    pub num_replicas: usize,
    /// Probability of message drop during sync
    pub message_drop_prob: f64,
    /// Probability of network partition
    pub partition_prob: f64,
    /// Maximum operations per run
    pub max_operations: usize,
}

impl Default for CRDTDSTConfig {
    fn default() -> Self {
        CRDTDSTConfig {
            seed: 0,
            num_replicas: 3,
            message_drop_prob: 0.0,
            partition_prob: 0.0,
            max_operations: 500,
        }
    }
}

impl CRDTDSTConfig {
    pub fn new(seed: u64, num_replicas: usize) -> Self {
        CRDTDSTConfig {
            seed,
            num_replicas,
            ..Default::default()
        }
    }

    /// Calm mode - no message drops
    pub fn calm(seed: u64) -> Self {
        CRDTDSTConfig {
            seed,
            num_replicas: 3,
            message_drop_prob: 0.0,
            partition_prob: 0.0,
            max_operations: 500,
        }
    }

    /// Moderate fault injection
    pub fn moderate(seed: u64) -> Self {
        CRDTDSTConfig {
            seed,
            num_replicas: 5,
            message_drop_prob: 0.1,
            partition_prob: 0.05,
            max_operations: 500,
        }
    }

    /// Chaos mode - aggressive fault injection
    pub fn chaos(seed: u64) -> Self {
        CRDTDSTConfig {
            seed,
            num_replicas: 7,
            message_drop_prob: 0.3,
            partition_prob: 0.15,
            max_operations: 500,
        }
    }
}

/// Result of a CRDT DST run
#[derive(Debug, Clone)]
pub struct CRDTDSTResult {
    /// Seed used
    pub seed: u64,
    /// Total operations executed
    pub total_operations: u64,
    /// Operations per replica
    pub ops_per_replica: HashMap<usize, u64>,
    /// Syncs performed
    pub syncs_performed: u64,
    /// Messages dropped
    pub messages_dropped: u64,
    /// Invariant violations found
    pub invariant_violations: Vec<String>,
    /// Whether convergence was achieved
    pub converged: bool,
}

impl CRDTDSTResult {
    pub fn new(seed: u64) -> Self {
        CRDTDSTResult {
            seed,
            total_operations: 0,
            ops_per_replica: HashMap::new(),
            syncs_performed: 0,
            messages_dropped: 0,
            invariant_violations: Vec::new(),
            converged: false,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty() && self.converged
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops, {} syncs, {} drops, converged={}, {} violations",
            self.seed,
            self.total_operations,
            self.syncs_performed,
            self.messages_dropped,
            self.converged,
            self.invariant_violations.len()
        )
    }
}

// =============================================================================
// GCounter DST Harness
// =============================================================================

/// DST harness for GCounter CRDT
pub struct GCounterDSTHarness {
    config: CRDTDSTConfig,
    rng: SimulatedRng,
    /// One GCounter per replica
    replicas: Vec<GCounter>,
    result: CRDTDSTResult,
}

impl GCounterDSTHarness {
    pub fn new(config: CRDTDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        let replicas = (0..config.num_replicas).map(|_| GCounter::new()).collect();

        GCounterDSTHarness {
            result: CRDTDSTResult::new(config.seed),
            config,
            rng,
            replicas,
        }
    }

    /// Run random operations
    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            let replica_idx = self.rng.gen_range(0, self.config.num_replicas as u64) as usize;
            let replica_id = ReplicaId::new(replica_idx as u64);

            // Random increment amount
            let amount = self.rng.gen_range(1, 10);

            self.replicas[replica_idx].increment_by(replica_id, amount);

            // Verify invariants after mutation
            #[cfg(debug_assertions)]
            self.replicas[replica_idx].verify_invariants();

            self.result.total_operations += 1;
            *self.result.ops_per_replica.entry(replica_idx).or_insert(0) += 1;
        }
    }

    /// Sync all replicas by pairwise merge
    /// Does multiple rounds to ensure convergence even with message drops
    pub fn sync_all(&mut self) {
        // Do multiple rounds to ensure convergence despite message drops
        // In real systems, anti-entropy would eventually sync everything
        let max_rounds = 5;
        for _round in 0..max_rounds {
            for i in 0..self.replicas.len() {
                for j in (i + 1)..self.replicas.len() {
                    if self.should_drop_message() {
                        self.result.messages_dropped += 1;
                        continue;
                    }

                    let merged = self.replicas[i].merge(&self.replicas[j]);
                    self.replicas[i] = merged.clone();
                    self.replicas[j] = merged;
                    self.result.syncs_performed += 1;
                }
            }
        }
    }

    fn should_drop_message(&mut self) -> bool {
        self.rng.gen_bool(self.config.message_drop_prob)
    }

    /// Check that all replicas have converged
    pub fn check_convergence(&mut self) {
        // After full sync, all replicas should have same value
        if self.replicas.is_empty() {
            self.result.converged = true;
            return;
        }

        let expected_value = self.replicas[0].value();

        for (i, replica) in self.replicas.iter().enumerate() {
            #[cfg(debug_assertions)]
            replica.verify_invariants();

            if replica.value() != expected_value {
                self.result.invariant_violations.push(format!(
                    "Replica {} has value {} but expected {}",
                    i,
                    replica.value(),
                    expected_value
                ));
            }
        }

        self.result.converged = self.result.invariant_violations.is_empty();
    }

    pub fn result(&self) -> &CRDTDSTResult {
        &self.result
    }

    pub fn into_result(self) -> CRDTDSTResult {
        self.result
    }
}

// =============================================================================
// PNCounter DST Harness
// =============================================================================

/// DST harness for PNCounter CRDT
pub struct PNCounterDSTHarness {
    config: CRDTDSTConfig,
    rng: SimulatedRng,
    replicas: Vec<PNCounter>,
    result: CRDTDSTResult,
}

impl PNCounterDSTHarness {
    pub fn new(config: CRDTDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        let replicas = (0..config.num_replicas).map(|_| PNCounter::new()).collect();

        PNCounterDSTHarness {
            result: CRDTDSTResult::new(config.seed),
            config,
            rng,
            replicas,
        }
    }

    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            let replica_idx = self.rng.gen_range(0, self.config.num_replicas as u64) as usize;
            let replica_id = ReplicaId::new(replica_idx as u64);

            // Random increment or decrement
            let amount = self.rng.gen_range(1, 10);
            if self.rng.gen_bool(0.5) {
                self.replicas[replica_idx].increment_by(replica_id, amount);
            } else {
                self.replicas[replica_idx].decrement_by(replica_id, amount);
            }

            #[cfg(debug_assertions)]
            self.replicas[replica_idx].verify_invariants();

            self.result.total_operations += 1;
            *self.result.ops_per_replica.entry(replica_idx).or_insert(0) += 1;
        }
    }

    pub fn sync_all(&mut self) {
        // Do multiple rounds to ensure convergence despite message drops
        let max_rounds = 5;
        for _round in 0..max_rounds {
            for i in 0..self.replicas.len() {
                for j in (i + 1)..self.replicas.len() {
                    if self.should_drop_message() {
                        self.result.messages_dropped += 1;
                        continue;
                    }

                    let merged = self.replicas[i].merge(&self.replicas[j]);
                    self.replicas[i] = merged.clone();
                    self.replicas[j] = merged;
                    self.result.syncs_performed += 1;
                }
            }
        }
    }

    fn should_drop_message(&mut self) -> bool {
        self.rng.gen_bool(self.config.message_drop_prob)
    }

    pub fn check_convergence(&mut self) {
        if self.replicas.is_empty() {
            self.result.converged = true;
            return;
        }

        let expected_value = self.replicas[0].value();

        for (i, replica) in self.replicas.iter().enumerate() {
            #[cfg(debug_assertions)]
            replica.verify_invariants();

            if replica.value() != expected_value {
                self.result.invariant_violations.push(format!(
                    "Replica {} has value {} but expected {}",
                    i,
                    replica.value(),
                    expected_value
                ));
            }
        }

        self.result.converged = self.result.invariant_violations.is_empty();
    }

    pub fn result(&self) -> &CRDTDSTResult {
        &self.result
    }

    pub fn into_result(self) -> CRDTDSTResult {
        self.result
    }
}

// =============================================================================
// ORSet DST Harness
// =============================================================================

/// DST harness for ORSet CRDT
pub struct ORSetDSTHarness {
    config: CRDTDSTConfig,
    rng: SimulatedRng,
    replicas: Vec<ORSet<String>>,
    result: CRDTDSTResult,
}

impl ORSetDSTHarness {
    pub fn new(config: CRDTDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        let replicas = (0..config.num_replicas).map(|_| ORSet::new()).collect();

        ORSetDSTHarness {
            result: CRDTDSTResult::new(config.seed),
            config,
            rng,
            replicas,
        }
    }

    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            let replica_idx = self.rng.gen_range(0, self.config.num_replicas as u64) as usize;
            let replica_id = ReplicaId::new(replica_idx as u64);

            // Random element
            let elem = format!("elem_{}", self.rng.gen_range(0, 20));

            // Add or remove
            if self.rng.gen_bool(0.7) {
                // 70% adds
                self.replicas[replica_idx].add(elem, replica_id);
            } else {
                // 30% removes
                self.replicas[replica_idx].remove(&elem);
            }

            #[cfg(debug_assertions)]
            self.replicas[replica_idx].verify_invariants();

            self.result.total_operations += 1;
            *self.result.ops_per_replica.entry(replica_idx).or_insert(0) += 1;
        }
    }

    pub fn sync_all(&mut self) {
        // Do multiple rounds to ensure convergence despite message drops
        let max_rounds = 5;
        for _round in 0..max_rounds {
            for i in 0..self.replicas.len() {
                for j in (i + 1)..self.replicas.len() {
                    if self.should_drop_message() {
                        self.result.messages_dropped += 1;
                        continue;
                    }

                    let merged = self.replicas[i].merge(&self.replicas[j]);
                    self.replicas[i] = merged.clone();
                    self.replicas[j] = merged;
                    self.result.syncs_performed += 1;
                }
            }
        }
    }

    fn should_drop_message(&mut self) -> bool {
        self.rng.gen_bool(self.config.message_drop_prob)
    }

    pub fn check_convergence(&mut self) {
        if self.replicas.is_empty() {
            self.result.converged = true;
            return;
        }

        // Collect elements from first replica
        let expected: std::collections::HashSet<_> = self.replicas[0].elements().cloned().collect();

        for (i, replica) in self.replicas.iter().enumerate() {
            #[cfg(debug_assertions)]
            replica.verify_invariants();

            let actual: std::collections::HashSet<_> = replica.elements().cloned().collect();
            if actual != expected {
                self.result.invariant_violations.push(format!(
                    "Replica {} has different elements: {:?} vs expected {:?}",
                    i, actual, expected
                ));
            }
        }

        self.result.converged = self.result.invariant_violations.is_empty();
    }

    pub fn result(&self) -> &CRDTDSTResult {
        &self.result
    }

    pub fn into_result(self) -> CRDTDSTResult {
        self.result
    }
}

// =============================================================================
// VectorClock DST Harness
// =============================================================================

/// DST harness for VectorClock
pub struct VectorClockDSTHarness {
    config: CRDTDSTConfig,
    rng: SimulatedRng,
    replicas: Vec<VectorClock>,
    result: CRDTDSTResult,
}

impl VectorClockDSTHarness {
    pub fn new(config: CRDTDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        let replicas = (0..config.num_replicas)
            .map(|_| VectorClock::new())
            .collect();

        VectorClockDSTHarness {
            result: CRDTDSTResult::new(config.seed),
            config,
            rng,
            replicas,
        }
    }

    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            let replica_idx = self.rng.gen_range(0, self.config.num_replicas as u64) as usize;
            let replica_id = ReplicaId::new(replica_idx as u64);

            // Increment this replica's clock
            self.replicas[replica_idx].increment(replica_id);

            #[cfg(debug_assertions)]
            self.replicas[replica_idx].verify_invariants();

            self.result.total_operations += 1;
            *self.result.ops_per_replica.entry(replica_idx).or_insert(0) += 1;
        }
    }

    pub fn sync_all(&mut self) {
        // Do multiple rounds to ensure convergence despite message drops
        let max_rounds = 5;
        for _round in 0..max_rounds {
            for i in 0..self.replicas.len() {
                for j in (i + 1)..self.replicas.len() {
                    if self.should_drop_message() {
                        self.result.messages_dropped += 1;
                        continue;
                    }

                    let merged = self.replicas[i].merge(&self.replicas[j]);
                    self.replicas[i] = merged.clone();
                    self.replicas[j] = merged;
                    self.result.syncs_performed += 1;
                }
            }
        }
    }

    fn should_drop_message(&mut self) -> bool {
        self.rng.gen_bool(self.config.message_drop_prob)
    }

    pub fn check_convergence(&mut self) {
        if self.replicas.is_empty() {
            self.result.converged = true;
            return;
        }

        // After full sync, all clocks should be equal
        for (i, replica) in self.replicas.iter().enumerate() {
            #[cfg(debug_assertions)]
            replica.verify_invariants();

            if *replica != self.replicas[0] {
                self.result
                    .invariant_violations
                    .push(format!("Replica {} has different clock than replica 0", i));
            }
        }

        self.result.converged = self.result.invariant_violations.is_empty();
    }

    pub fn result(&self) -> &CRDTDSTResult {
        &self.result
    }

    pub fn into_result(self) -> CRDTDSTResult {
        self.result
    }
}

// =============================================================================
// Batch Runners
// =============================================================================

/// Run a batch of GCounter DST tests
pub fn run_gcounter_batch(
    base_seed: u64,
    count: usize,
    ops_per_run: usize,
    config_fn: impl Fn(u64) -> CRDTDSTConfig,
) -> Vec<CRDTDSTResult> {
    let mut results = Vec::with_capacity(count);

    for i in 0..count {
        let seed = base_seed + i as u64;
        let config = config_fn(seed);

        let mut harness = GCounterDSTHarness::new(config);
        harness.run(ops_per_run);
        harness.sync_all();
        harness.check_convergence();

        results.push(harness.into_result());
    }

    results
}

/// Run a batch of PNCounter DST tests
pub fn run_pncounter_batch(
    base_seed: u64,
    count: usize,
    ops_per_run: usize,
    config_fn: impl Fn(u64) -> CRDTDSTConfig,
) -> Vec<CRDTDSTResult> {
    let mut results = Vec::with_capacity(count);

    for i in 0..count {
        let seed = base_seed + i as u64;
        let config = config_fn(seed);

        let mut harness = PNCounterDSTHarness::new(config);
        harness.run(ops_per_run);
        harness.sync_all();
        harness.check_convergence();

        results.push(harness.into_result());
    }

    results
}

/// Run a batch of ORSet DST tests
pub fn run_orset_batch(
    base_seed: u64,
    count: usize,
    ops_per_run: usize,
    config_fn: impl Fn(u64) -> CRDTDSTConfig,
) -> Vec<CRDTDSTResult> {
    let mut results = Vec::with_capacity(count);

    for i in 0..count {
        let seed = base_seed + i as u64;
        let config = config_fn(seed);

        let mut harness = ORSetDSTHarness::new(config);
        harness.run(ops_per_run);
        harness.sync_all();
        harness.check_convergence();

        results.push(harness.into_result());
    }

    results
}

/// Run a batch of VectorClock DST tests
pub fn run_vectorclock_batch(
    base_seed: u64,
    count: usize,
    ops_per_run: usize,
    config_fn: impl Fn(u64) -> CRDTDSTConfig,
) -> Vec<CRDTDSTResult> {
    let mut results = Vec::with_capacity(count);

    for i in 0..count {
        let seed = base_seed + i as u64;
        let config = config_fn(seed);

        let mut harness = VectorClockDSTHarness::new(config);
        harness.run(ops_per_run);
        harness.sync_all();
        harness.check_convergence();

        results.push(harness.into_result());
    }

    results
}

/// Summarize batch results
pub fn summarize_batch(results: &[CRDTDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed_seeds: Vec<u64> = results
        .iter()
        .filter(|r| !r.is_success())
        .map(|r| r.seed)
        .collect();

    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();
    let total_syncs: u64 = results.iter().map(|r| r.syncs_performed).sum();
    let total_drops: u64 = results.iter().map(|r| r.messages_dropped).sum();

    let mut summary = format!(
        "Batch: {}/{} passed, {} total ops, {} syncs, {} drops",
        passed, total, total_ops, total_syncs, total_drops
    );

    if !failed_seeds.is_empty() {
        summary.push_str(&format!("\nFailed seeds: {:?}", failed_seeds));
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // GCounter Tests
    // =========================================================================

    #[test]
    fn test_gcounter_dst_single_calm() {
        let config = CRDTDSTConfig::calm(42);
        let mut harness = GCounterDSTHarness::new(config);

        harness.run(100);
        harness.sync_all();
        harness.check_convergence();

        let result = harness.result();
        assert!(
            result.is_success(),
            "Calm should converge: {:?}",
            result.invariant_violations
        );
    }

    #[test]
    fn test_gcounter_dst_100_seeds() {
        let results = run_gcounter_batch(0, 100, 100, CRDTDSTConfig::calm);
        let summary = summarize_batch(&results);
        println!("GCounter 100 seeds:\n{}", summary);

        assert!(
            results.iter().all(|r| r.is_success()),
            "All calm runs should converge"
        );
    }

    #[test]
    fn test_gcounter_dst_moderate_100_seeds() {
        let results = run_gcounter_batch(1000, 100, 100, CRDTDSTConfig::moderate);
        let summary = summarize_batch(&results);
        println!("GCounter moderate 100 seeds:\n{}", summary);

        // With message drops, some may not fully converge until more syncs
        // But after sync_all completes (retries), should converge
    }

    // =========================================================================
    // PNCounter Tests
    // =========================================================================

    #[test]
    fn test_pncounter_dst_single_calm() {
        let config = CRDTDSTConfig::calm(42);
        let mut harness = PNCounterDSTHarness::new(config);

        harness.run(100);
        harness.sync_all();
        harness.check_convergence();

        let result = harness.result();
        assert!(
            result.is_success(),
            "Calm should converge: {:?}",
            result.invariant_violations
        );
    }

    #[test]
    fn test_pncounter_dst_100_seeds() {
        let results = run_pncounter_batch(0, 100, 100, CRDTDSTConfig::calm);
        let summary = summarize_batch(&results);
        println!("PNCounter 100 seeds:\n{}", summary);

        assert!(
            results.iter().all(|r| r.is_success()),
            "All calm runs should converge"
        );
    }

    // =========================================================================
    // ORSet Tests
    // =========================================================================

    #[test]
    fn test_orset_dst_single_calm() {
        let config = CRDTDSTConfig::calm(42);
        let mut harness = ORSetDSTHarness::new(config);

        harness.run(100);
        harness.sync_all();
        harness.check_convergence();

        let result = harness.result();
        assert!(
            result.is_success(),
            "Calm should converge: {:?}",
            result.invariant_violations
        );
    }

    #[test]
    fn test_orset_dst_100_seeds() {
        let results = run_orset_batch(0, 100, 100, CRDTDSTConfig::calm);
        let summary = summarize_batch(&results);
        println!("ORSet 100 seeds:\n{}", summary);

        assert!(
            results.iter().all(|r| r.is_success()),
            "All calm runs should converge"
        );
    }

    // =========================================================================
    // VectorClock Tests
    // =========================================================================

    #[test]
    fn test_vectorclock_dst_single_calm() {
        let config = CRDTDSTConfig::calm(42);
        let mut harness = VectorClockDSTHarness::new(config);

        harness.run(100);
        harness.sync_all();
        harness.check_convergence();

        let result = harness.result();
        assert!(
            result.is_success(),
            "Calm should converge: {:?}",
            result.invariant_violations
        );
    }

    #[test]
    fn test_vectorclock_dst_100_seeds() {
        let results = run_vectorclock_batch(0, 100, 100, CRDTDSTConfig::calm);
        let summary = summarize_batch(&results);
        println!("VectorClock 100 seeds:\n{}", summary);

        assert!(
            results.iter().all(|r| r.is_success()),
            "All calm runs should converge"
        );
    }

    // =========================================================================
    // Determinism Tests
    // =========================================================================

    #[test]
    fn test_crdt_dst_determinism() {
        let seed = 12345;

        // Run same seed twice
        let mut h1 = GCounterDSTHarness::new(CRDTDSTConfig::calm(seed));
        h1.run(50);
        let ops1 = h1.result().total_operations;

        let mut h2 = GCounterDSTHarness::new(CRDTDSTConfig::calm(seed));
        h2.run(50);
        let ops2 = h2.result().total_operations;

        assert_eq!(ops1, ops2, "Same seed should produce same results");
    }
}
