//! Transaction Deterministic Simulation Tests
//!
//! VOPR-style tests for MULTI/EXEC/WATCH/DISCARD semantics.
//! These tests verify transaction invariants under interleaved
//! client operations with multiple seeds.

use redis_sim::redis::{
    run_transaction_batch, summarize_transaction_batch, TransactionDSTConfig,
    TransactionDSTHarness,
};

// =============================================================================
// Standard Configuration Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_transaction_dst_100_seeds_standard() {
    let results = run_transaction_batch(0, 100, 200, TransactionDSTConfig::new);
    let summary = summarize_transaction_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed, 100,
        "All 100 seeds should pass with standard config"
    );
}

#[test]
fn test_transaction_dst_100_seeds_high_conflict() {
    let results = run_transaction_batch(1000, 100, 200, TransactionDSTConfig::high_conflict);
    let summary = summarize_transaction_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 high-conflict seeds should pass");

    // Verify conflicts were actually tested
    let total_conflicts: u64 = results.iter().map(|r| r.watch_conflict).sum();
    assert!(
        total_conflicts > 50,
        "High-conflict config should produce many conflicts, got {}",
        total_conflicts
    );
}

#[test]
fn test_transaction_dst_100_seeds_error_heavy() {
    let results = run_transaction_batch(2000, 100, 200, TransactionDSTConfig::error_heavy);
    let summary = summarize_transaction_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 error-heavy seeds should pass");

    let total_errors: u64 = results.iter().map(|r| r.error_scenarios).sum();
    assert!(
        total_errors > 100,
        "Error-heavy config should produce many error scenarios, got {}",
        total_errors
    );
}

// =============================================================================
// Stress Tests
// =============================================================================

#[test]
fn test_transaction_dst_stress_1000_ops() {
    let mut harness = TransactionDSTHarness::with_seed(42);
    harness.run(1000);
    let result = harness.result();
    println!("Stress 1000 ops: {}", result.summary());
    assert!(result.is_success(), "1000 ops should maintain invariants");
}

#[test]
fn test_transaction_dst_stress_2000_ops_high_conflict() {
    let config = TransactionDSTConfig::high_conflict(99999);
    let mut harness = TransactionDSTHarness::new(config);
    harness.run(2000);
    let result = harness.result();
    println!(
        "Stress high conflict 2000 ops: {} (conflicts: {}, no_conflict: {})",
        result.summary(),
        result.watch_conflict,
        result.watch_no_conflict
    );
    assert!(
        result.is_success(),
        "High conflict stress should maintain invariants"
    );
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_transaction_dst_tiny_keyspace() {
    let config = TransactionDSTConfig {
        seed: 77777,
        num_keys: 2, // Only 2 keys!
        conflict_prob: 0.5,
        discard_prob: 0.1,
        error_prob: 0.1,
    };

    let mut harness = TransactionDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Tiny keyspace (2 keys): {}", result.summary());
    assert!(
        result.is_success(),
        "Tiny keyspace should maintain invariants"
    );
}

#[test]
fn test_transaction_dst_all_scenarios_exercised() {
    let config = TransactionDSTConfig::new(42);
    let mut harness = TransactionDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();

    println!("{}", result.summary());
    assert!(result.is_success());

    // Verify all scenario types were exercised
    assert!(
        result.watch_no_conflict > 0,
        "Should exercise watch-no-conflict"
    );
    assert!(result.watch_conflict > 0, "Should exercise watch-conflict");
    assert!(result.simple_exec > 0, "Should exercise simple exec");
    assert!(result.discards > 0, "Should exercise discards");
    assert!(result.error_scenarios > 0, "Should exercise error scenarios");
}

// =============================================================================
// Mixed Configuration Tests
// =============================================================================

#[test]
fn test_transaction_dst_50_seeds_mixed_configs() {
    let mut all_passed = true;
    let mut failures = Vec::new();

    for seed in 0..50 {
        let config = match seed % 3 {
            0 => TransactionDSTConfig::new(seed),
            1 => TransactionDSTConfig::high_conflict(seed),
            _ => TransactionDSTConfig::error_heavy(seed),
        };

        let mut harness = TransactionDSTHarness::new(config);
        harness.run(200);
        let result = harness.result();

        if !result.is_success() {
            all_passed = false;
            failures.push(result.clone());
        }
    }

    if !all_passed {
        for f in &failures {
            println!("FAILED: {}", f.summary());
            for v in &f.invariant_violations {
                println!("  {}", v);
            }
        }
    }

    assert!(all_passed, "{} seeds failed", failures.len());
}

// =============================================================================
// Longer Tests (ignored by default for CI speed)
// =============================================================================

#[test]
#[ignore]
fn test_transaction_dst_500_seeds() {
    let results = run_transaction_batch(0, 500, 200, TransactionDSTConfig::new);
    let summary = summarize_transaction_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 500, "All 500 seeds should pass");
}

#[test]
#[ignore]
fn test_transaction_dst_stress_5000_ops() {
    let mut harness = TransactionDSTHarness::with_seed(31415);
    harness.run(5000);
    let result = harness.result();
    println!("Stress 5000 ops: {}", result.summary());
    assert!(result.is_success(), "5000 ops should maintain invariants");
}
