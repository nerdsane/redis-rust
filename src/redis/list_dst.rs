//! Deterministic Simulation Testing for Lists
//!
//! VOPR-style testing harness for RedisList that enables:
//! - Deterministic random operation generation
//! - Invariant checking after each operation
//! - Seed-based reproducibility for debugging

use super::data::{RedisList, SDS};
use crate::io::simulation::SimulatedRng;
use crate::io::Rng;

/// Configuration for List DST
#[derive(Debug, Clone)]
pub struct ListDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of unique values to use
    pub num_values: usize,
    /// Probability of pop operation (vs push)
    pub pop_prob: f64,
    /// Probability of using lpush/lpop (vs rpush/rpop)
    pub left_prob: f64,
    /// Probability of lset operation
    pub lset_prob: f64,
    /// Probability of trim operation
    pub trim_prob: f64,
}

impl Default for ListDSTConfig {
    fn default() -> Self {
        ListDSTConfig {
            seed: 0,
            num_values: 100,
            pop_prob: 0.3,
            left_prob: 0.5,
            lset_prob: 0.05,
            trim_prob: 0.02,
        }
    }
}

impl ListDSTConfig {
    pub fn new(seed: u64) -> Self {
        ListDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Configuration with high churn (lots of push/pop)
    pub fn high_churn(seed: u64) -> Self {
        ListDSTConfig {
            seed,
            num_values: 50,
            pop_prob: 0.45,
            left_prob: 0.5,
            lset_prob: 0.02,
            trim_prob: 0.01,
        }
    }

    /// Configuration focusing on modifications (lset, trim)
    pub fn modify_heavy(seed: u64) -> Self {
        ListDSTConfig {
            seed,
            num_values: 100,
            pop_prob: 0.2,
            left_prob: 0.5,
            lset_prob: 0.15,
            trim_prob: 0.05,
        }
    }
}

/// Operation type for logging
#[derive(Debug, Clone)]
pub enum ListOp {
    LPush { value: String },
    RPush { value: String },
    LPop,
    RPop,
    LSet { index: isize, value: String },
    Trim { start: isize, stop: isize },
}

/// Result of a List DST run
#[derive(Debug, Clone)]
pub struct ListDSTResult {
    pub seed: u64,
    pub total_operations: u64,
    pub lpushes: u64,
    pub rpushes: u64,
    pub lpops: u64,
    pub rpops: u64,
    pub lsets: u64,
    pub trims: u64,
    pub invariant_violations: Vec<String>,
    pub last_op: Option<ListOp>,
}

impl ListDSTResult {
    pub fn new(seed: u64) -> Self {
        ListDSTResult {
            seed,
            total_operations: 0,
            lpushes: 0,
            rpushes: 0,
            lpops: 0,
            rpops: 0,
            lsets: 0,
            trims: 0,
            invariant_violations: Vec::new(),
            last_op: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops (lpush:{}, rpush:{}, lpop:{}, rpop:{}, lset:{}, trim:{}), {} violations",
            self.seed,
            self.total_operations,
            self.lpushes,
            self.rpushes,
            self.lpops,
            self.rpops,
            self.lsets,
            self.trims,
            self.invariant_violations.len()
        )
    }
}

/// DST harness for RedisList
pub struct ListDSTHarness {
    config: ListDSTConfig,
    rng: SimulatedRng,
    list: RedisList,
    result: ListDSTResult,
    /// Track expected length for cross-checking
    expected_len: usize,
}

impl ListDSTHarness {
    pub fn new(config: ListDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        ListDSTHarness {
            result: ListDSTResult::new(config.seed),
            config,
            rng,
            list: RedisList::new(),
            expected_len: 0,
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self::new(ListDSTConfig::new(seed))
    }

    fn random_value(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.config.num_values as u64);
        format!("value:{}", idx)
    }

    fn run_single_op(&mut self) {
        let op_type = self.rng.gen_range(0, 100);
        let trim_threshold = (self.config.trim_prob * 100.0) as u64;
        let lset_threshold = trim_threshold + (self.config.lset_prob * 100.0) as u64;
        let pop_threshold = lset_threshold + (self.config.pop_prob * 100.0) as u64;

        if op_type < trim_threshold && !self.list.is_empty() {
            // Trim operation
            let len = self.list.len() as isize;
            let start = self.rng.gen_range(0, len as u64) as isize;
            let stop = self.rng.gen_range(start as u64, len as u64) as isize;
            self.result.last_op = Some(ListOp::Trim { start, stop });
            self.list.trim(start, stop);
            self.expected_len = (stop - start + 1).max(0) as usize;
            self.result.trims += 1;
        } else if op_type < lset_threshold && !self.list.is_empty() {
            // LSet operation
            let len = self.list.len() as isize;
            let index = self.rng.gen_range(0, len as u64) as isize;
            let value = self.random_value();
            self.result.last_op = Some(ListOp::LSet {
                index,
                value: value.clone(),
            });
            let _ = self.list.set(index, SDS::from_str(&value));
            // Length doesn't change
            self.result.lsets += 1;
        } else if op_type < pop_threshold && !self.list.is_empty() {
            // Pop operation
            let use_left = self.rng.gen_range(0, 100) < (self.config.left_prob * 100.0) as u64;
            if use_left {
                self.result.last_op = Some(ListOp::LPop);
                self.list.lpop();
                self.result.lpops += 1;
            } else {
                self.result.last_op = Some(ListOp::RPop);
                self.list.rpop();
                self.result.rpops += 1;
            }
            self.expected_len = self.expected_len.saturating_sub(1);
        } else {
            // Push operation
            let value = self.random_value();
            let use_left = self.rng.gen_range(0, 100) < (self.config.left_prob * 100.0) as u64;
            if use_left {
                self.result.last_op = Some(ListOp::LPush {
                    value: value.clone(),
                });
                self.list.lpush(SDS::from_str(&value));
                self.result.lpushes += 1;
            } else {
                self.result.last_op = Some(ListOp::RPush {
                    value: value.clone(),
                });
                self.list.rpush(SDS::from_str(&value));
                self.result.rpushes += 1;
            }
            self.expected_len += 1;
        }

        self.result.total_operations += 1;

        if let Err(violation) = self.check_invariants() {
            self.result.invariant_violations.push(format!(
                "Op #{}: {:?} - {}",
                self.result.total_operations, self.result.last_op, violation
            ));
        }
    }

    fn check_invariants(&self) -> Result<(), String> {
        // Invariant 1: Length must match expected
        if self.list.len() != self.expected_len {
            return Err(format!(
                "Length mismatch: actual={}, expected={}",
                self.list.len(),
                self.expected_len
            ));
        }

        // Invariant 2: is_empty consistency
        if self.list.is_empty() != (self.expected_len == 0) {
            return Err(format!(
                "is_empty mismatch: is_empty={}, expected_len={}",
                self.list.is_empty(),
                self.expected_len
            ));
        }

        // Invariant 3: Range should return correct number of elements
        let range = self.list.range(0, -1);
        if range.len() != self.expected_len {
            return Err(format!(
                "Range length mismatch: range.len={}, expected={}",
                range.len(),
                self.expected_len
            ));
        }

        // Invariant 4: Each index should be accessible
        for i in 0..self.expected_len {
            if self.list.get(i as isize).is_none() {
                return Err(format!("Index {} not accessible but should be", i));
            }
        }

        Ok(())
    }

    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            self.run_single_op();
            if !self.result.invariant_violations.is_empty() {
                break;
            }
        }
    }

    pub fn result(&self) -> &ListDSTResult {
        &self.result
    }

    pub fn list(&self) -> &RedisList {
        &self.list
    }
}

/// Run a batch of DST tests
pub fn run_list_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> ListDSTConfig,
) -> Vec<ListDSTResult> {
    (0..num_seeds)
        .map(|i| {
            let seed = start_seed + i as u64;
            let config = config_fn(seed);
            let mut harness = ListDSTHarness::new(config);
            harness.run(ops_per_seed);
            harness.result().clone()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_list_batch(results: &[ListDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed = total - passed;
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();

    let mut summary = format!(
        "List DST Summary\n\
         ================\n\
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
    fn test_list_dst_single_seed() {
        let mut harness = ListDSTHarness::with_seed(12345);
        harness.run(100);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success(), "Seed 12345 failed");
    }

    #[test]
    fn test_list_dst_high_churn() {
        let config = ListDSTConfig::high_churn(42);
        let mut harness = ListDSTHarness::new(config);
        harness.run(500);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success());
    }

    #[test]
    fn test_list_dst_10_seeds() {
        let results = run_list_batch(0, 10, 500, ListDSTConfig::new);
        let summary = summarize_list_batch(&results);
        println!("{}", summary);

        let passed = results.iter().filter(|r| r.is_success()).count();
        assert_eq!(passed, 10, "All 10 seeds should pass");
    }
}
