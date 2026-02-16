//! Deterministic Simulation Testing for Transactions (MULTI/EXEC/WATCH)
//!
//! Shadow-state testing harness for Redis transaction semantics, specifically:
//! - MULTI/EXEC atomicity (all-or-nothing execution)
//! - WATCH/UNWATCH optimistic locking (conflict detection)
//! - DISCARD behavior
//! - Error handling (nested MULTI, EXEC without MULTI, etc.)
//!
//! ## Design
//!
//! Two simulated clients share a single `CommandExecutor`, interleaving commands
//! to test WATCH conflict detection. The harness generates interleaved operation
//! sequences and verifies transaction invariants after each sequence.
//!
//! ## Usage
//!
//! ```rust,ignore
//! for seed in 0..100 {
//!     let mut harness = TransactionDSTHarness::with_seed(seed);
//!     harness.run(200);
//!     assert!(harness.result().is_success(), "Seed {} failed", seed);
//! }
//! ```

use super::command::Command;
use super::data::SDS;
use super::executor::CommandExecutor;
use super::resp::RespValue;
use crate::io::simulation::SimulatedRng;
use crate::io::Rng;

/// Configuration for Transaction DST
#[derive(Debug, Clone)]
pub struct TransactionDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of unique keys
    pub num_keys: usize,
    /// Probability of WATCH conflict scenario
    pub conflict_prob: f64,
    /// Probability of DISCARD scenario
    pub discard_prob: f64,
    /// Probability of error scenario (nested MULTI, etc.)
    pub error_prob: f64,
}

impl Default for TransactionDSTConfig {
    fn default() -> Self {
        TransactionDSTConfig {
            seed: 0,
            num_keys: 20,
            conflict_prob: 0.3,
            discard_prob: 0.15,
            error_prob: 0.1,
        }
    }
}

impl TransactionDSTConfig {
    pub fn new(seed: u64) -> Self {
        TransactionDSTConfig {
            seed,
            ..Default::default()
        }
    }

    pub fn high_conflict(seed: u64) -> Self {
        TransactionDSTConfig {
            seed,
            num_keys: 5, // Small key space = more conflicts
            conflict_prob: 0.6,
            discard_prob: 0.1,
            error_prob: 0.05,
        }
    }

    pub fn error_heavy(seed: u64) -> Self {
        TransactionDSTConfig {
            seed,
            error_prob: 0.3,
            discard_prob: 0.2,
            ..Default::default()
        }
    }
}

/// Operation type for logging
#[derive(Debug, Clone)]
pub enum TransactionOp {
    WatchExecNoConflict(String),
    WatchExecConflict(String),
    MultiExecSimple(String),
    DiscardAfterMulti(String),
    ErrorScenario(String),
    UnwatchThenExec(String),
}

/// Result of a Transaction DST run
#[derive(Debug, Clone)]
pub struct TransactionDSTResult {
    pub seed: u64,
    pub total_operations: u64,
    pub watch_no_conflict: u64,
    pub watch_conflict: u64,
    pub simple_exec: u64,
    pub discards: u64,
    pub error_scenarios: u64,
    pub unwatch_scenarios: u64,
    pub invariant_violations: Vec<String>,
    pub last_op: Option<TransactionOp>,
}

impl TransactionDSTResult {
    pub fn new(seed: u64) -> Self {
        TransactionDSTResult {
            seed,
            total_operations: 0,
            watch_no_conflict: 0,
            watch_conflict: 0,
            simple_exec: 0,
            discards: 0,
            error_scenarios: 0,
            unwatch_scenarios: 0,
            invariant_violations: Vec::new(),
            last_op: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops (no_conflict:{}, conflict:{}, exec:{}, discard:{}, error:{}, unwatch:{}), {} violations",
            self.seed,
            self.total_operations,
            self.watch_no_conflict,
            self.watch_conflict,
            self.simple_exec,
            self.discards,
            self.error_scenarios,
            self.unwatch_scenarios,
            self.invariant_violations.len()
        )
    }
}

/// DST harness for Transaction semantics
pub struct TransactionDSTHarness {
    config: TransactionDSTConfig,
    rng: SimulatedRng,
    executor: CommandExecutor,
    result: TransactionDSTResult,
}

impl TransactionDSTHarness {
    pub fn new(config: TransactionDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        TransactionDSTHarness {
            result: TransactionDSTResult::new(config.seed),
            config,
            rng,
            executor: CommandExecutor::new(),
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self::new(TransactionDSTConfig::new(seed))
    }

    fn random_key(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.config.num_keys as u64);
        format!("txkey:{}", idx)
    }

    fn random_value(&mut self) -> Vec<u8> {
        let idx = self.rng.gen_range(0, 100);
        format!("txval:{}", idx).into_bytes()
    }

    // =========================================================================
    // Scenario Runners
    // =========================================================================

    fn run_single_op(&mut self) {
        let roll = self.rng.gen_range(0, 100);

        let error_threshold = (self.config.error_prob * 100.0) as u64;
        let discard_threshold = error_threshold + (self.config.discard_prob * 100.0) as u64;
        let conflict_threshold = discard_threshold + (self.config.conflict_prob * 100.0) as u64;

        if roll < error_threshold {
            self.run_error_scenario();
        } else if roll < discard_threshold {
            self.run_discard_scenario();
        } else if roll < conflict_threshold {
            self.run_watch_conflict_scenario();
        } else if roll < conflict_threshold + 15 {
            self.run_unwatch_scenario();
        } else {
            // Remaining: either watch-no-conflict or simple exec
            if self.rng.gen_range(0, 2) == 0 {
                self.run_watch_no_conflict_scenario();
            } else {
                self.run_simple_exec_scenario();
            }
        }
    }

    /// Scenario: WATCH + no mutation -> EXEC succeeds
    fn run_watch_no_conflict_scenario(&mut self) {
        let key = self.random_key();
        let value = self.random_value();
        let new_value = self.random_value();

        let desc = format!("WATCH {} no-conflict", key);
        self.result.last_op = Some(TransactionOp::WatchExecNoConflict(desc));
        self.result.watch_no_conflict += 1;

        // Setup: ensure key exists
        self.executor
            .execute(&Command::set(key.clone(), SDS::new(value.clone())));

        // Client A: WATCH key
        let watch_resp = self
            .executor
            .execute(&Command::Watch(vec![key.clone()]));
        self.assert_ok(&watch_resp, "WATCH should return OK");

        // No mutation happens between WATCH and MULTI/EXEC

        // Client A: MULTI
        let multi_resp = self.executor.execute(&Command::Multi);
        self.assert_ok(&multi_resp, "MULTI should return OK");

        // Client A: SET key new_value (queued)
        let queued_resp = self.executor.execute(&Command::set(
            key.clone(),
            SDS::new(new_value.clone()),
        ));
        self.assert_queued(&queued_resp, "SET inside MULTI should be QUEUED");

        // Client A: EXEC (should succeed - no conflict)
        let exec_resp = self.executor.execute(&Command::Exec);

        // Invariant 3: WATCH + no mutation -> EXEC succeeds
        match &exec_resp {
            RespValue::Array(Some(results)) => {
                if results.len() != 1 {
                    self.violation(&format!(
                        "EXEC should return 1 result, got {}",
                        results.len()
                    ));
                }
                // Verify the SET was applied
                let get_resp = self.executor.execute(&Command::Get(key.clone()));
                self.assert_bulk_eq(
                    &get_resp,
                    &new_value,
                    "GET after successful EXEC should return new value",
                );
            }
            RespValue::BulkString(None) => {
                self.violation("EXEC returned nil but no conflict occurred");
            }
            _ => {
                self.violation(&format!("EXEC returned unexpected: {:?}", exec_resp));
            }
        }
    }

    /// Scenario: WATCH + mutation by "other client" -> EXEC returns nil
    fn run_watch_conflict_scenario(&mut self) {
        let key = self.random_key();
        let value = self.random_value();
        // Ensure conflict_value differs from value to guarantee WATCH detects it
        let mut conflict_value = self.random_value();
        if conflict_value == value {
            conflict_value = format!("{}_conflict", String::from_utf8_lossy(&value)).into_bytes();
        }
        let new_value = self.random_value();

        let desc = format!("WATCH {} conflict", key);
        self.result.last_op = Some(TransactionOp::WatchExecConflict(desc));
        self.result.watch_conflict += 1;

        // Setup: ensure key exists
        self.executor
            .execute(&Command::set(key.clone(), SDS::new(value)));

        // Client A: WATCH key
        let watch_resp = self
            .executor
            .execute(&Command::Watch(vec![key.clone()]));
        self.assert_ok(&watch_resp, "WATCH should return OK");

        // Client B: SET key conflict_value (mutation between WATCH and EXEC)
        // Since we share the same executor, this simulates another client
        self.executor
            .execute(&Command::set(key.clone(), SDS::new(conflict_value.clone())));

        // Client A: MULTI
        let multi_resp = self.executor.execute(&Command::Multi);
        self.assert_ok(&multi_resp, "MULTI should return OK");

        // Client A: SET key new_value (queued)
        let queued_resp = self.executor.execute(&Command::set(
            key.clone(),
            SDS::new(new_value),
        ));
        self.assert_queued(&queued_resp, "SET inside MULTI should be QUEUED");

        // Client A: EXEC (should fail - conflict detected)
        let exec_resp = self.executor.execute(&Command::Exec);

        // Invariant 4: WATCH + mutation -> EXEC returns nil
        match &exec_resp {
            RespValue::BulkString(None) => {
                // Correct! WATCH detected the conflict.
                // Verify the conflict value is still there (transaction was aborted)
                let get_resp = self.executor.execute(&Command::Get(key.clone()));
                self.assert_bulk_eq(
                    &get_resp,
                    &conflict_value,
                    "GET after aborted EXEC should return conflict value",
                );
            }
            RespValue::Array(Some(_)) => {
                self.violation("EXEC succeeded despite WATCH conflict - should have returned nil");
            }
            _ => {
                self.violation(&format!(
                    "EXEC returned unexpected on conflict: {:?}",
                    exec_resp
                ));
            }
        }
    }

    /// Scenario: Simple MULTI/EXEC without WATCH
    fn run_simple_exec_scenario(&mut self) {
        let key1 = self.random_key();
        let key2 = self.random_key();
        let val1 = self.random_value();
        let val2 = self.random_value();

        let desc = format!("MULTI/EXEC {} {}", key1, key2);
        self.result.last_op = Some(TransactionOp::MultiExecSimple(desc));
        self.result.simple_exec += 1;

        // MULTI
        let multi_resp = self.executor.execute(&Command::Multi);
        self.assert_ok(&multi_resp, "MULTI should return OK");

        // Queue SET key1 val1
        let q1 = self
            .executor
            .execute(&Command::set(key1.clone(), SDS::new(val1.clone())));
        self.assert_queued(&q1, "First queued SET");

        // Queue SET key2 val2
        let q2 = self
            .executor
            .execute(&Command::set(key2.clone(), SDS::new(val2.clone())));
        self.assert_queued(&q2, "Second queued SET");

        // EXEC
        let exec_resp = self.executor.execute(&Command::Exec);

        // Invariant 10: MULTI/EXEC atomicity - all commands executed
        match &exec_resp {
            RespValue::Array(Some(results)) => {
                if results.len() != 2 {
                    self.violation(&format!(
                        "EXEC should return 2 results, got {}",
                        results.len()
                    ));
                }
                // Verify SETs were applied (if key1==key2, second SET wins)
                if key1 == key2 {
                    let get = self.executor.execute(&Command::Get(key1));
                    self.assert_bulk_eq(&get, &val2, "GET key after EXEC (duplicate key, last wins)");
                } else {
                    let get1 = self.executor.execute(&Command::Get(key1));
                    self.assert_bulk_eq(&get1, &val1, "GET key1 after EXEC");
                    let get2 = self.executor.execute(&Command::Get(key2));
                    self.assert_bulk_eq(&get2, &val2, "GET key2 after EXEC");
                }
            }
            _ => {
                self.violation(&format!("EXEC returned unexpected: {:?}", exec_resp));
            }
        }
    }

    /// Scenario: MULTI then DISCARD
    fn run_discard_scenario(&mut self) {
        let key = self.random_key();
        let old_value = self.random_value();
        let new_value = self.random_value();

        let desc = format!("DISCARD {}", key);
        self.result.last_op = Some(TransactionOp::DiscardAfterMulti(desc));
        self.result.discards += 1;

        // Setup: set a known value
        self.executor
            .execute(&Command::set(key.clone(), SDS::new(old_value.clone())));

        // MULTI
        let multi_resp = self.executor.execute(&Command::Multi);
        self.assert_ok(&multi_resp, "MULTI should return OK");

        // Queue a SET
        let q = self
            .executor
            .execute(&Command::set(key.clone(), SDS::new(new_value)));
        self.assert_queued(&q, "Queued SET before DISCARD");

        // DISCARD
        let discard_resp = self.executor.execute(&Command::Discard);
        self.assert_ok(&discard_resp, "DISCARD should return OK");

        // Verify: key should still have old value (queued SET was discarded)
        let get_resp = self.executor.execute(&Command::Get(key.clone()));
        self.assert_bulk_eq(
            &get_resp,
            &old_value,
            "GET after DISCARD should return old value",
        );

        // Invariant 1: EXEC after DISCARD returns error
        let exec_resp = self.executor.execute(&Command::Exec);
        self.assert_error_contains(
            &exec_resp,
            "EXEC without MULTI",
            "EXEC after DISCARD should error",
        );
    }

    /// Scenario: Error conditions (nested MULTI, EXEC without MULTI, etc.)
    fn run_error_scenario(&mut self) {
        let sub = self.rng.gen_range(0, 4);
        self.result.error_scenarios += 1;

        match sub {
            0 => {
                // Invariant 2: MULTI nesting returns error
                let desc = "nested MULTI".to_string();
                self.result.last_op = Some(TransactionOp::ErrorScenario(desc));

                let m1 = self.executor.execute(&Command::Multi);
                self.assert_ok(&m1, "First MULTI should return OK");

                let m2 = self.executor.execute(&Command::Multi);
                self.assert_error_contains(&m2, "nested", "Nested MULTI should error");

                // Clean up: DISCARD or EXEC
                self.executor.execute(&Command::Discard);
            }
            1 => {
                // EXEC without MULTI
                let desc = "EXEC without MULTI".to_string();
                self.result.last_op = Some(TransactionOp::ErrorScenario(desc));

                let resp = self.executor.execute(&Command::Exec);
                self.assert_error_contains(
                    &resp,
                    "EXEC without MULTI",
                    "EXEC without MULTI should error",
                );
            }
            2 => {
                // DISCARD without MULTI
                let desc = "DISCARD without MULTI".to_string();
                self.result.last_op = Some(TransactionOp::ErrorScenario(desc));

                let resp = self.executor.execute(&Command::Discard);
                self.assert_error_contains(
                    &resp,
                    "DISCARD without MULTI",
                    "DISCARD without MULTI should error",
                );
            }
            _ => {
                // WATCH inside MULTI should error
                let desc = "WATCH inside MULTI".to_string();
                self.result.last_op = Some(TransactionOp::ErrorScenario(desc));

                let m = self.executor.execute(&Command::Multi);
                self.assert_ok(&m, "MULTI should return OK");

                let key = self.random_key();
                let w = self
                    .executor
                    .execute(&Command::Watch(vec![key]));
                self.assert_error_contains(&w, "WATCH inside MULTI", "WATCH inside MULTI should error");

                // Clean up
                self.executor.execute(&Command::Discard);
            }
        }
    }

    /// Scenario: UNWATCH clears all watches -> EXEC succeeds even with mutation
    fn run_unwatch_scenario(&mut self) {
        let key = self.random_key();
        let value = self.random_value();
        let conflict_value = self.random_value();
        let new_value = self.random_value();

        let desc = format!("UNWATCH then EXEC {}", key);
        self.result.last_op = Some(TransactionOp::UnwatchThenExec(desc));
        self.result.unwatch_scenarios += 1;

        // Setup
        self.executor
            .execute(&Command::set(key.clone(), SDS::new(value)));

        // WATCH
        self.executor
            .execute(&Command::Watch(vec![key.clone()]));

        // Invariant 5: UNWATCH clears all watches
        let unwatch_resp = self.executor.execute(&Command::Unwatch);
        self.assert_ok(&unwatch_resp, "UNWATCH should return OK");

        // Mutate the key (would normally cause conflict, but we unwatched)
        self.executor
            .execute(&Command::set(key.clone(), SDS::new(conflict_value)));

        // MULTI/EXEC should succeed because we unwatched
        let multi_resp = self.executor.execute(&Command::Multi);
        self.assert_ok(&multi_resp, "MULTI after UNWATCH should return OK");

        let q = self
            .executor
            .execute(&Command::set(key.clone(), SDS::new(new_value.clone())));
        self.assert_queued(&q, "Queued SET after UNWATCH");

        let exec_resp = self.executor.execute(&Command::Exec);
        match &exec_resp {
            RespValue::Array(Some(results)) => {
                if results.len() != 1 {
                    self.violation(&format!(
                        "EXEC after UNWATCH should return 1 result, got {}",
                        results.len()
                    ));
                }
                // Verify SET was applied
                let get_resp = self.executor.execute(&Command::Get(key));
                self.assert_bulk_eq(
                    &get_resp,
                    &new_value,
                    "GET after UNWATCH + EXEC should return new value",
                );
            }
            RespValue::BulkString(None) => {
                self.violation("EXEC returned nil after UNWATCH - should have succeeded");
            }
            _ => {
                self.violation(&format!(
                    "EXEC after UNWATCH returned unexpected: {:?}",
                    exec_resp
                ));
            }
        }
    }

    // =========================================================================
    // Assertion Helpers
    // =========================================================================

    fn violation(&mut self, msg: &str) {
        self.result.invariant_violations.push(format!(
            "Op #{}: {:?} - {}",
            self.result.total_operations, self.result.last_op, msg
        ));
    }

    fn assert_ok(&mut self, resp: &RespValue, context: &str) {
        if !matches!(resp, RespValue::SimpleString(s) if s.as_ref() == "OK") {
            self.violation(&format!("{}: expected OK, got {:?}", context, resp));
        }
    }

    fn assert_queued(&mut self, resp: &RespValue, context: &str) {
        if !matches!(resp, RespValue::SimpleString(s) if s.as_ref() == "QUEUED") {
            self.violation(&format!("{}: expected QUEUED, got {:?}", context, resp));
        }
    }

    fn assert_bulk_eq(&mut self, resp: &RespValue, expected: &[u8], context: &str) {
        match resp {
            RespValue::BulkString(Some(data)) if data == expected => {}
            _ => {
                self.violation(&format!(
                    "{}: expected BulkString({:?}), got {:?}",
                    context,
                    String::from_utf8_lossy(expected),
                    resp
                ));
            }
        }
    }

    fn assert_error_contains(&mut self, resp: &RespValue, substring: &str, context: &str) {
        match resp {
            RespValue::Error(e) if e.as_ref().contains(substring) => {}
            _ => {
                self.violation(&format!(
                    "{}: expected Error containing '{}', got {:?}",
                    context, substring, resp
                ));
            }
        }
    }

    // =========================================================================
    // Public API
    // =========================================================================

    /// Run specified number of operations
    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            self.result.total_operations += 1;
            self.run_single_op();

            if !self.result.invariant_violations.is_empty() {
                break;
            }
        }
    }

    /// Get the result
    pub fn result(&self) -> &TransactionDSTResult {
        &self.result
    }
}

/// Run a batch of transaction DST tests
pub fn run_transaction_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> TransactionDSTConfig,
) -> Vec<TransactionDSTResult> {
    (0..num_seeds)
        .map(|i| {
            let seed = start_seed + i as u64;
            let config = config_fn(seed);
            let mut harness = TransactionDSTHarness::new(config);
            harness.run(ops_per_seed);
            harness.result().clone()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_transaction_batch(results: &[TransactionDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed = total - passed;
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();

    let mut summary = format!(
        "Transaction DST Summary\n\
         =======================\n\
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
    fn test_transaction_dst_single_seed() {
        let mut harness = TransactionDSTHarness::with_seed(12345);
        harness.run(100);
        let result = harness.result();
        println!("{}", result.summary());
        for v in &result.invariant_violations {
            println!("  VIOLATION: {}", v);
        }
        assert!(result.is_success(), "Seed 12345 failed");
    }

    #[test]
    fn test_transaction_dst_high_conflict() {
        let config = TransactionDSTConfig::high_conflict(42);
        let mut harness = TransactionDSTHarness::new(config);
        harness.run(200);
        let result = harness.result();
        println!("High conflict: {}", result.summary());
        for v in &result.invariant_violations {
            println!("  VIOLATION: {}", v);
        }
        assert!(result.is_success());
        assert!(
            result.watch_conflict > 0,
            "High conflict config should produce conflicts"
        );
    }

    #[test]
    fn test_transaction_dst_error_heavy() {
        let config = TransactionDSTConfig::error_heavy(99);
        let mut harness = TransactionDSTHarness::new(config);
        harness.run(200);
        let result = harness.result();
        println!("Error heavy: {}", result.summary());
        assert!(result.is_success());
        assert!(
            result.error_scenarios > 0,
            "Error-heavy config should produce error scenarios"
        );
    }

    #[test]
    fn test_transaction_dst_10_seeds() {
        let results = run_transaction_batch(0, 10, 200, TransactionDSTConfig::new);
        let summary = summarize_transaction_batch(&results);
        println!("{}", summary);

        let passed = results.iter().filter(|r| r.is_success()).count();
        assert_eq!(passed, 10, "All 10 seeds should pass");
    }
}
