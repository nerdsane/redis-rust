//! Executor Deterministic Simulation Tests
//!
//! DST tests for CommandExecutor with multiple seeds.
//! These tests verify that the executor maintains invariants across
//! ALL command types under various random operation sequences.

use redis_sim::redis::{
    run_executor_batch, summarize_executor_batch, ExecutorDSTConfig, ExecutorDSTHarness,
};

// =============================================================================
// Calm Tests (100 ops) - Quick Smoke Tests
// =============================================================================

#[test]
fn test_executor_dst_calm_100_seeds() {
    let results = run_executor_batch(0, 100, 100, ExecutorDSTConfig::calm);
    let summary = summarize_executor_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 calm seeds should pass");
}

// =============================================================================
// Moderate Tests (1000 ops) - Standard Coverage
// =============================================================================

#[test]
fn test_executor_dst_moderate_100_seeds() {
    let results = run_executor_batch(0, 100, 1000, ExecutorDSTConfig::new);
    let summary = summarize_executor_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed, 100,
        "All 100 moderate seeds should pass"
    );
}

#[test]
fn test_executor_dst_moderate_string_heavy_50_seeds() {
    let results = run_executor_batch(500, 50, 1000, ExecutorDSTConfig::string_heavy);
    let summary = summarize_executor_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 50, "All 50 string-heavy seeds should pass");
}

// =============================================================================
// Chaos Tests (5000 ops) - Stress Testing
// =============================================================================

#[test]
fn test_executor_dst_chaos_50_seeds() {
    let results = run_executor_batch(0, 50, 5000, ExecutorDSTConfig::chaos);
    let summary = summarize_executor_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 50, "All 50 chaos seeds should pass");
}

// =============================================================================
// Stress Tests - High Operation Count
// =============================================================================

#[test]
fn test_executor_dst_stress_1000_ops() {
    let mut harness = ExecutorDSTHarness::with_seed(42);
    harness.run(1000);
    let result = harness.result();
    println!("Stress 1000 ops: {}", result.summary());
    assert!(result.is_success(), "1000 ops should maintain invariants");
}

#[test]
fn test_executor_dst_stress_5000_ops() {
    let mut harness = ExecutorDSTHarness::with_seed(12345);
    harness.run(5000);
    let result = harness.result();
    println!("Stress 5000 ops: {}", result.summary());
    assert!(result.is_success(), "5000 ops should maintain invariants");
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_executor_dst_tiny_keyspace() {
    // Very small key space = lots of type conflicts and overwrites
    let config = ExecutorDSTConfig {
        seed: 88888,
        num_keys: 3,
        num_values: 5,
        num_fields: 3,
        ..Default::default()
    };

    let mut harness = ExecutorDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Tiny keyspace (3 keys): {}", result.summary());
    assert!(
        result.is_success(),
        "Tiny keyspace should maintain invariants"
    );
}

#[test]
fn test_executor_dst_all_categories_exercised() {
    let config = ExecutorDSTConfig::new(42);
    let mut harness = ExecutorDSTHarness::new(config);
    harness.run(2000);
    let result = harness.result();

    println!("{}", result.summary());
    assert!(result.is_success());

    // Verify all command categories were exercised
    assert!(result.string_ops > 0, "String ops should be exercised");
    assert!(result.key_ops > 0, "Key ops should be exercised");
    assert!(result.list_ops > 0, "List ops should be exercised");
    assert!(result.set_ops > 0, "Set ops should be exercised");
    assert!(result.hash_ops > 0, "Hash ops should be exercised");
    assert!(result.sorted_set_ops > 0, "Sorted set ops should be exercised");
    assert!(result.expiry_ops > 0, "Expiry ops should be exercised");
}

// =============================================================================
// Mixed Configuration Tests
// =============================================================================

#[test]
fn test_executor_dst_50_seeds_mixed_configs() {
    let mut all_passed = true;
    let mut failures = Vec::new();

    for seed in 0..50 {
        let config = match seed % 4 {
            0 => ExecutorDSTConfig::new(seed),
            1 => ExecutorDSTConfig::calm(seed),
            2 => ExecutorDSTConfig::chaos(seed),
            _ => ExecutorDSTConfig::string_heavy(seed),
        };

        let mut harness = ExecutorDSTHarness::new(config);
        harness.run(500);
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
fn test_executor_dst_500_seeds() {
    let results = run_executor_batch(0, 500, 1000, ExecutorDSTConfig::new);
    let summary = summarize_executor_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 500, "All 500 seeds should pass");
}

#[test]
#[ignore]
fn test_executor_dst_stress_10000_ops() {
    let mut harness = ExecutorDSTHarness::with_seed(31415);
    harness.run(10000);
    let result = harness.result();
    println!("Stress 10000 ops: {}", result.summary());
    assert!(result.is_success(), "10000 ops should maintain invariants");
}
