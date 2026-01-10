//! Deterministic Simulation Testing for Hashes
//!
//! VOPR-style testing harness for RedisHash that enables:
//! - Deterministic random operation generation
//! - Invariant checking after each operation
//! - Seed-based reproducibility for debugging

use super::data::{RedisHash, SDS};
use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use std::collections::HashSet;

/// Configuration for Hash DST
#[derive(Debug, Clone)]
pub struct HashDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of unique field names to use
    pub num_fields: usize,
    /// Number of unique values to use
    pub num_values: usize,
    /// Probability of delete operation
    pub delete_prob: f64,
    /// Probability of update (set existing field)
    pub update_prob: f64,
}

impl Default for HashDSTConfig {
    fn default() -> Self {
        HashDSTConfig {
            seed: 0,
            num_fields: 100,
            num_values: 50,
            delete_prob: 0.15,
            update_prob: 0.3,
        }
    }
}

impl HashDSTConfig {
    pub fn new(seed: u64) -> Self {
        HashDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Configuration with small field space (more collisions)
    pub fn small_fields(seed: u64) -> Self {
        HashDSTConfig {
            seed,
            num_fields: 10,
            num_values: 20,
            delete_prob: 0.2,
            update_prob: 0.5,
        }
    }

    /// Configuration with high churn (lots of deletes)
    pub fn high_churn(seed: u64) -> Self {
        HashDSTConfig {
            seed,
            num_fields: 50,
            num_values: 30,
            delete_prob: 0.4,
            update_prob: 0.3,
        }
    }
}

/// Operation type for logging
#[derive(Debug, Clone)]
pub enum HashOp {
    Set { field: String, value: String },
    Delete { field: String },
}

/// Result of a Hash DST run
#[derive(Debug, Clone)]
pub struct HashDSTResult {
    pub seed: u64,
    pub total_operations: u64,
    pub sets: u64,
    pub updates: u64,
    pub deletes: u64,
    pub invariant_violations: Vec<String>,
    pub last_op: Option<HashOp>,
}

impl HashDSTResult {
    pub fn new(seed: u64) -> Self {
        HashDSTResult {
            seed,
            total_operations: 0,
            sets: 0,
            updates: 0,
            deletes: 0,
            invariant_violations: Vec::new(),
            last_op: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops (sets:{}, updates:{}, deletes:{}), {} violations",
            self.seed,
            self.total_operations,
            self.sets,
            self.updates,
            self.deletes,
            self.invariant_violations.len()
        )
    }
}

/// DST harness for RedisHash
pub struct HashDSTHarness {
    config: HashDSTConfig,
    rng: SimulatedRng,
    hash: RedisHash,
    result: HashDSTResult,
    /// Track expected fields and their values for cross-checking
    expected_fields: HashSet<String>,
}

impl HashDSTHarness {
    pub fn new(config: HashDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        HashDSTHarness {
            result: HashDSTResult::new(config.seed),
            config,
            rng,
            hash: RedisHash::new(),
            expected_fields: HashSet::new(),
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self::new(HashDSTConfig::new(seed))
    }

    fn random_field(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.config.num_fields as u64);
        format!("field:{}", idx)
    }

    fn random_value(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.config.num_values as u64);
        format!("value:{}", idx)
    }

    fn run_single_op(&mut self) {
        let op_type = self.rng.gen_range(0, 100);
        let delete_threshold = (self.config.delete_prob * 100.0) as u64;

        if op_type < delete_threshold && !self.expected_fields.is_empty() {
            // Delete operation - pick an existing field
            let field = self.random_field();
            self.result.last_op = Some(HashOp::Delete {
                field: field.clone(),
            });
            let existed = self.expected_fields.remove(&field);
            self.hash.delete(&SDS::from_str(&field));
            if existed {
                self.result.deletes += 1;
            }
        } else {
            // Set operation
            let field = self.random_field();
            let value = self.random_value();
            self.result.last_op = Some(HashOp::Set {
                field: field.clone(),
                value: value.clone(),
            });

            let is_update = self.expected_fields.contains(&field);
            self.hash.set(SDS::from_str(&field), SDS::from_str(&value));
            self.expected_fields.insert(field);

            if is_update {
                self.result.updates += 1;
            } else {
                self.result.sets += 1;
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
        // Invariant 1: Length must match expected fields count
        if self.hash.len() != self.expected_fields.len() {
            return Err(format!(
                "Length mismatch: actual={}, expected={}",
                self.hash.len(),
                self.expected_fields.len()
            ));
        }

        // Invariant 2: is_empty consistency
        if self.hash.is_empty() != self.expected_fields.is_empty() {
            return Err(format!(
                "is_empty mismatch: is_empty={}, expected_empty={}",
                self.hash.is_empty(),
                self.expected_fields.is_empty()
            ));
        }

        // Invariant 3: All expected fields should exist
        for field in &self.expected_fields {
            if !self.hash.exists(&SDS::from_str(field)) {
                return Err(format!("Expected field '{}' not found", field));
            }
        }

        // Invariant 4: Keys should match expected fields
        let keys: HashSet<String> = self.hash.keys().iter().map(|k| k.to_string()).collect();
        if keys != self.expected_fields {
            let missing: Vec<_> = self.expected_fields.difference(&keys).collect();
            let extra: Vec<_> = keys.difference(&self.expected_fields).collect();
            return Err(format!(
                "Keys mismatch: missing={:?}, extra={:?}",
                missing, extra
            ));
        }

        // Invariant 5: Values should be retrievable for all keys
        for field in &self.expected_fields {
            if self.hash.get(&SDS::from_str(field)).is_none() {
                return Err(format!("Field '{}' exists but get returns None", field));
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

    pub fn result(&self) -> &HashDSTResult {
        &self.result
    }

    pub fn hash(&self) -> &RedisHash {
        &self.hash
    }
}

/// Run a batch of DST tests
pub fn run_hash_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> HashDSTConfig,
) -> Vec<HashDSTResult> {
    (0..num_seeds)
        .map(|i| {
            let seed = start_seed + i as u64;
            let config = config_fn(seed);
            let mut harness = HashDSTHarness::new(config);
            harness.run(ops_per_seed);
            harness.result().clone()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_hash_batch(results: &[HashDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed = total - passed;
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();

    let mut summary = format!(
        "Hash DST Summary\n\
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
    fn test_hash_dst_single_seed() {
        let mut harness = HashDSTHarness::with_seed(12345);
        harness.run(100);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success(), "Seed 12345 failed");
    }

    #[test]
    fn test_hash_dst_small_fields() {
        let config = HashDSTConfig::small_fields(42);
        let mut harness = HashDSTHarness::new(config);
        harness.run(500);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success());
    }

    #[test]
    fn test_hash_dst_10_seeds() {
        let results = run_hash_batch(0, 10, 500, HashDSTConfig::new);
        let summary = summarize_hash_batch(&results);
        println!("{}", summary);

        let passed = results.iter().filter(|r| r.is_success()).count();
        assert_eq!(passed, 10, "All 10 seeds should pass");
    }
}
