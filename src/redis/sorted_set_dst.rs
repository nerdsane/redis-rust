//! Deterministic Simulation Testing for Sorted Sets
//!
//! VOPR-style testing harness for RedisSortedSet that enables:
//! - Deterministic random operation generation
//! - Invariant checking after each operation
//! - Seed-based reproducibility for debugging
//!
//! ## Usage
//!
//! ```rust,ignore
//! for seed in 0..100 {
//!     let mut harness = SortedSetDSTHarness::new(seed);
//!     harness.run(500);
//!     assert!(harness.result().is_success(), "Seed {} failed", seed);
//! }
//! ```

use super::data::{RedisSortedSet, SDS};
use crate::io::simulation::SimulatedRng;
use crate::io::Rng;

/// Configuration for Sorted Set DST
#[derive(Debug, Clone)]
pub struct SortedSetDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of unique keys to use (creates a bounded key space)
    pub num_keys: usize,
    /// Probability of update operation (vs add new)
    pub update_prob: f64,
    /// Probability of remove operation
    pub remove_prob: f64,
    /// Maximum score value
    pub max_score: f64,
}

impl Default for SortedSetDSTConfig {
    fn default() -> Self {
        SortedSetDSTConfig {
            seed: 0,
            num_keys: 100,
            update_prob: 0.3,
            remove_prob: 0.1,
            max_score: 1000.0,
        }
    }
}

impl SortedSetDSTConfig {
    /// Standard configuration with given seed
    pub fn new(seed: u64) -> Self {
        SortedSetDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Configuration with smaller key space (more updates)
    pub fn small_keyspace(seed: u64) -> Self {
        SortedSetDSTConfig {
            seed,
            num_keys: 10,
            update_prob: 0.5,
            remove_prob: 0.2,
            max_score: 100.0,
        }
    }

    /// Configuration with large key space (more adds)
    pub fn large_keyspace(seed: u64) -> Self {
        SortedSetDSTConfig {
            seed,
            num_keys: 1000,
            update_prob: 0.1,
            remove_prob: 0.05,
            max_score: 10000.0,
        }
    }
}

/// Operation type for logging
#[derive(Debug, Clone)]
pub enum SortedSetOp {
    Add { member: String, score: f64 },
    Remove { member: String },
}

/// Result of a Sorted Set DST run
#[derive(Debug, Clone)]
pub struct SortedSetDSTResult {
    /// Seed used
    pub seed: u64,
    /// Total operations executed
    pub total_operations: u64,
    /// Add operations
    pub adds: u64,
    /// Update operations (add with existing member)
    pub updates: u64,
    /// Remove operations
    pub removes: u64,
    /// Invariant violations found (with operation context)
    pub invariant_violations: Vec<String>,
    /// Last operation before failure (if any)
    pub last_op: Option<SortedSetOp>,
}

impl SortedSetDSTResult {
    pub fn new(seed: u64) -> Self {
        SortedSetDSTResult {
            seed,
            total_operations: 0,
            adds: 0,
            updates: 0,
            removes: 0,
            invariant_violations: Vec::new(),
            last_op: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops ({} adds, {} updates, {} removes), {} violations",
            self.seed,
            self.total_operations,
            self.adds,
            self.updates,
            self.removes,
            self.invariant_violations.len()
        )
    }
}

/// DST harness for RedisSortedSet
pub struct SortedSetDSTHarness {
    config: SortedSetDSTConfig,
    rng: SimulatedRng,
    sorted_set: RedisSortedSet,
    result: SortedSetDSTResult,
}

impl SortedSetDSTHarness {
    pub fn new(config: SortedSetDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        SortedSetDSTHarness {
            result: SortedSetDSTResult::new(config.seed),
            config,
            rng,
            sorted_set: RedisSortedSet::new(),
        }
    }

    /// Create with just a seed (uses default config)
    pub fn with_seed(seed: u64) -> Self {
        Self::new(SortedSetDSTConfig::new(seed))
    }

    /// Generate a random member key
    fn random_member(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.config.num_keys as u64);
        format!("member:{}", idx)
    }

    /// Generate a random score
    fn random_score(&mut self) -> f64 {
        let raw = self
            .rng
            .gen_range(0, (self.config.max_score * 100.0) as u64);
        raw as f64 / 100.0
    }

    /// Run a single random operation
    fn run_single_op(&mut self) {
        let op_type = self.rng.gen_range(0, 100);

        if op_type < (self.config.remove_prob * 100.0) as u64 {
            // Remove operation
            let member = self.random_member();
            self.result.last_op = Some(SortedSetOp::Remove {
                member: member.clone(),
            });
            self.sorted_set.remove(&SDS::from_str(&member));
            self.result.removes += 1;
        } else {
            // Add/update operation
            let member = self.random_member();
            let score = self.random_score();
            self.result.last_op = Some(SortedSetOp::Add {
                member: member.clone(),
                score,
            });

            let is_new = self.sorted_set.add(SDS::from_str(&member), score);
            if is_new {
                self.result.adds += 1;
            } else {
                self.result.updates += 1;
            }
        }

        self.result.total_operations += 1;

        // Verify invariants after each operation
        if let Err(violation) = self.check_invariants() {
            self.result.invariant_violations.push(format!(
                "Op #{}: {:?} - {}",
                self.result.total_operations, self.result.last_op, violation
            ));
        }
    }

    /// Check all invariants
    fn check_invariants(&self) -> Result<(), String> {
        // Invariant 1: members hashmap and skiplist must have same length
        let members_len = self.sorted_set.len();
        let skiplist_len = self.sorted_set.skiplist_len();

        if members_len != skiplist_len {
            return Err(format!(
                "Length mismatch: members={}, skiplist={}",
                members_len, skiplist_len
            ));
        }

        // Invariant 2: Check that the sorted set is properly sorted
        if !self.sorted_set.is_sorted() {
            return Err("Sorted set is not properly sorted".to_string());
        }

        // Invariant 3: Every member in range should have consistent score
        let range = self.sorted_set.range(0, -1);
        for (member, score) in &range {
            let member_str = member.to_string();
            let lookup_score = self.sorted_set.score(member);
            match lookup_score {
                None => {
                    return Err(format!(
                        "Member '{}' in range but not found via score lookup",
                        member_str
                    ));
                }
                Some(s) if (s - score).abs() > f64::EPSILON => {
                    return Err(format!(
                        "Score mismatch for '{}': range={}, lookup={}",
                        member_str, score, s
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Run specified number of operations
    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            self.run_single_op();

            // Stop early if we hit a violation
            if !self.result.invariant_violations.is_empty() {
                break;
            }
        }
    }

    /// Get the result
    pub fn result(&self) -> &SortedSetDSTResult {
        &self.result
    }

    /// Get the sorted set for inspection
    pub fn sorted_set(&self) -> &RedisSortedSet {
        &self.sorted_set
    }
}

/// Run a batch of DST tests with different seeds
pub fn run_sorted_set_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> SortedSetDSTConfig,
) -> Vec<SortedSetDSTResult> {
    (0..num_seeds)
        .map(|i| {
            let seed = start_seed + i as u64;
            let config = config_fn(seed);
            let mut harness = SortedSetDSTHarness::new(config);
            harness.run(ops_per_seed);
            harness.result().clone()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_batch(results: &[SortedSetDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed = total - passed;
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();

    let mut summary = format!(
        "Sorted Set DST Summary\n\
         ======================\n\
         Seeds: {} total, {} passed, {} failed\n\
         Total operations: {}\n",
        total, passed, failed, total_ops
    );

    if failed > 0 {
        summary.push_str("\nFailed seeds:\n");
        for result in results.iter().filter(|r| !r.is_success()) {
            summary.push_str(&format!("  Seed {}: {}\n", result.seed, result.summary()));
            for violation in &result.invariant_violations {
                summary.push_str(&format!("    - {}\n", violation));
            }
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sorted_set_dst_single_seed() {
        let mut harness = SortedSetDSTHarness::with_seed(12345);
        harness.run(100);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success(), "Seed 12345 failed");
    }

    #[test]
    fn test_sorted_set_dst_small_keyspace() {
        // Small keyspace means more updates/removes
        let config = SortedSetDSTConfig::small_keyspace(42);
        let mut harness = SortedSetDSTHarness::new(config);
        harness.run(500);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success());
    }

    #[test]
    fn test_sorted_set_dst_10_seeds() {
        let results = run_sorted_set_batch(0, 10, 500, SortedSetDSTConfig::new);
        let summary = summarize_batch(&results);
        println!("{}", summary);

        let passed = results.iter().filter(|r| r.is_success()).count();
        assert_eq!(passed, 10, "All 10 seeds should pass");
    }
}
