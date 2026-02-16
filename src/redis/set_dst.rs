//! Deterministic Simulation Testing for Sets
//!
//! Shadow-state testing harness for RedisSet that enables:
//! - Deterministic random operation generation
//! - Invariant checking after each operation
//! - Seed-based reproducibility for debugging

use super::data::{RedisSet, SDS};
use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use std::collections::HashSet;

/// Configuration for Set DST
#[derive(Debug, Clone)]
pub struct SetDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of unique members to use
    pub num_members: usize,
    /// Probability of remove operation
    pub remove_prob: f64,
}

impl Default for SetDSTConfig {
    fn default() -> Self {
        SetDSTConfig {
            seed: 0,
            num_members: 100,
            remove_prob: 0.25,
        }
    }
}

impl SetDSTConfig {
    pub fn new(seed: u64) -> Self {
        SetDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Configuration with small member space (more collisions)
    pub fn small_members(seed: u64) -> Self {
        SetDSTConfig {
            seed,
            num_members: 10,
            remove_prob: 0.3,
        }
    }

    /// Configuration with high churn (lots of removes)
    pub fn high_churn(seed: u64) -> Self {
        SetDSTConfig {
            seed,
            num_members: 50,
            remove_prob: 0.45,
        }
    }

    /// Configuration with large member space
    pub fn large_members(seed: u64) -> Self {
        SetDSTConfig {
            seed,
            num_members: 500,
            remove_prob: 0.15,
        }
    }
}

/// Operation type for logging
#[derive(Debug, Clone)]
pub enum SetOp {
    Add { member: String },
    Remove { member: String },
}

/// Result of a Set DST run
#[derive(Debug, Clone)]
pub struct SetDSTResult {
    pub seed: u64,
    pub total_operations: u64,
    pub adds: u64,
    pub add_existed: u64,
    pub removes: u64,
    pub remove_not_found: u64,
    pub invariant_violations: Vec<String>,
    pub last_op: Option<SetOp>,
}

impl SetDSTResult {
    pub fn new(seed: u64) -> Self {
        SetDSTResult {
            seed,
            total_operations: 0,
            adds: 0,
            add_existed: 0,
            removes: 0,
            remove_not_found: 0,
            invariant_violations: Vec::new(),
            last_op: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops (adds:{}, existed:{}, removes:{}, not_found:{}), {} violations",
            self.seed,
            self.total_operations,
            self.adds,
            self.add_existed,
            self.removes,
            self.remove_not_found,
            self.invariant_violations.len()
        )
    }
}

/// DST harness for RedisSet
pub struct SetDSTHarness {
    config: SetDSTConfig,
    rng: SimulatedRng,
    set: RedisSet,
    result: SetDSTResult,
    /// Track expected members for cross-checking
    expected_members: HashSet<String>,
}

impl SetDSTHarness {
    pub fn new(config: SetDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        SetDSTHarness {
            result: SetDSTResult::new(config.seed),
            config,
            rng,
            set: RedisSet::new(),
            expected_members: HashSet::new(),
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self::new(SetDSTConfig::new(seed))
    }

    fn random_member(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.config.num_members as u64);
        format!("member:{}", idx)
    }

    fn run_single_op(&mut self) {
        let op_type = self.rng.gen_range(0, 100);
        let remove_threshold = (self.config.remove_prob * 100.0) as u64;

        if op_type < remove_threshold {
            // Remove operation
            let member = self.random_member();
            self.result.last_op = Some(SetOp::Remove {
                member: member.clone(),
            });
            let existed = self.expected_members.remove(&member);
            let removed = self.set.remove(&SDS::from_str(&member));

            if removed {
                self.result.removes += 1;
            } else {
                self.result.remove_not_found += 1;
            }

            // Verify consistency between expected and actual
            if existed != removed {
                self.result.invariant_violations.push(format!(
                    "Remove mismatch: expected existed={}, actual removed={}",
                    existed, removed
                ));
            }
        } else {
            // Add operation
            let member = self.random_member();
            self.result.last_op = Some(SetOp::Add {
                member: member.clone(),
            });

            let already_exists = self.expected_members.contains(&member);
            self.expected_members.insert(member.clone());
            let inserted = self.set.add(SDS::from_str(&member));

            if inserted {
                self.result.adds += 1;
            } else {
                self.result.add_existed += 1;
            }

            // Verify consistency
            if already_exists == inserted {
                self.result.invariant_violations.push(format!(
                    "Add mismatch: already_exists={}, inserted={}",
                    already_exists, inserted
                ));
            }
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
        // Invariant 1: Length must match expected members count
        if self.set.len() != self.expected_members.len() {
            return Err(format!(
                "Length mismatch: actual={}, expected={}",
                self.set.len(),
                self.expected_members.len()
            ));
        }

        // Invariant 2: is_empty consistency
        if self.set.is_empty() != self.expected_members.is_empty() {
            return Err(format!(
                "is_empty mismatch: is_empty={}, expected_empty={}",
                self.set.is_empty(),
                self.expected_members.is_empty()
            ));
        }

        // Invariant 3: All expected members should exist
        for member in &self.expected_members {
            if !self.set.contains(&SDS::from_str(member)) {
                return Err(format!("Expected member '{}' not found", member));
            }
        }

        // Invariant 4: Members should match expected
        let actual_members: HashSet<String> =
            self.set.members().iter().map(|m| m.to_string()).collect();
        if actual_members != self.expected_members {
            let missing: Vec<_> = self.expected_members.difference(&actual_members).collect();
            let extra: Vec<_> = actual_members.difference(&self.expected_members).collect();
            return Err(format!(
                "Members mismatch: missing={:?}, extra={:?}",
                missing, extra
            ));
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

    pub fn result(&self) -> &SetDSTResult {
        &self.result
    }

    pub fn set(&self) -> &RedisSet {
        &self.set
    }
}

/// Run a batch of DST tests
pub fn run_set_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> SetDSTConfig,
) -> Vec<SetDSTResult> {
    (0..num_seeds)
        .map(|i| {
            let seed = start_seed + i as u64;
            let config = config_fn(seed);
            let mut harness = SetDSTHarness::new(config);
            harness.run(ops_per_seed);
            harness.result().clone()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_set_batch(results: &[SetDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed = total - passed;
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();

    let mut summary = format!(
        "Set DST Summary\n\
         ===============\n\
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
    fn test_set_dst_single_seed() {
        let mut harness = SetDSTHarness::with_seed(12345);
        harness.run(100);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success(), "Seed 12345 failed");
    }

    #[test]
    fn test_set_dst_small_members() {
        let config = SetDSTConfig::small_members(42);
        let mut harness = SetDSTHarness::new(config);
        harness.run(500);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success());
    }

    #[test]
    fn test_set_dst_10_seeds() {
        let results = run_set_batch(0, 10, 500, SetDSTConfig::new);
        let summary = summarize_set_batch(&results);
        println!("{}", summary);

        let passed = results.iter().filter(|r| r.is_success()).count();
        assert_eq!(passed, 10, "All 10 seeds should pass");
    }
}
