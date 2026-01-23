//! List Deterministic Simulation Tests
//!
//! VOPR-style tests for RedisList with multiple seeds.

use redis_sim::redis::{run_list_batch, summarize_list_batch, ListDSTConfig, ListDSTHarness};

// =============================================================================
// Standard Configuration Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_list_dst_100_seeds_standard() {
    let results = run_list_batch(0, 100, 500, ListDSTConfig::new);
    let summary = summarize_list_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed, 100,
        "All 100 seeds should pass with standard config"
    );
}

#[test]
fn test_list_dst_100_seeds_high_churn() {
    let results = run_list_batch(1000, 100, 500, ListDSTConfig::high_churn);
    let summary = summarize_list_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with high churn");
}

#[test]
fn test_list_dst_100_seeds_modify_heavy() {
    let results = run_list_batch(2000, 100, 500, ListDSTConfig::modify_heavy);
    let summary = summarize_list_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with modify heavy");
}

// =============================================================================
// Stress Tests
// =============================================================================

#[test]
fn test_list_dst_stress_1000_ops() {
    let mut harness = ListDSTHarness::with_seed(42);
    harness.run(1000);
    let result = harness.result();
    println!("Stress 1000 ops: {}", result.summary());
    assert!(result.is_success(), "1000 ops should maintain invariants");
}

#[test]
fn test_list_dst_stress_5000_ops() {
    let mut harness = ListDSTHarness::with_seed(12345);
    harness.run(5000);
    let result = harness.result();
    println!("Stress 5000 ops: {}", result.summary());
    assert!(result.is_success(), "5000 ops should maintain invariants");
}

#[test]
fn test_list_dst_stress_high_churn_2000_ops() {
    let config = ListDSTConfig::high_churn(99999);
    let mut harness = ListDSTHarness::new(config);
    harness.run(2000);
    let result = harness.result();
    println!(
        "Stress high churn 2000 ops: {} (lpop:{}, rpop:{})",
        result.summary(),
        result.lpops,
        result.rpops
    );
    assert!(
        result.is_success(),
        "High churn stress should maintain invariants"
    );
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_list_dst_mostly_pops() {
    // Configuration with very high pop rate
    let config = ListDSTConfig {
        seed: 77777,
        num_values: 50,
        pop_prob: 0.6,
        left_prob: 0.5,
        lset_prob: 0.02,
        trim_prob: 0.01,
    };

    let mut harness = ListDSTHarness::new(config);
    harness.run(1000);
    let result = harness.result();
    println!("Mostly pops: {}", result.summary());
    assert!(
        result.is_success(),
        "Mostly pops should maintain invariants"
    );
}

#[test]
fn test_list_dst_heavy_trim() {
    // Configuration with lots of trim operations
    let config = ListDSTConfig {
        seed: 88888,
        num_values: 30,
        pop_prob: 0.1,
        left_prob: 0.5,
        lset_prob: 0.05,
        trim_prob: 0.15,
    };

    let mut harness = ListDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Heavy trim: {} (trims: {})", result.summary(), result.trims);
    assert!(result.is_success(), "Heavy trim should maintain invariants");
}

#[test]
fn test_list_dst_heavy_lset() {
    // Configuration with lots of lset operations
    let config = ListDSTConfig {
        seed: 11111,
        num_values: 50,
        pop_prob: 0.15,
        left_prob: 0.5,
        lset_prob: 0.25,
        trim_prob: 0.02,
    };

    let mut harness = ListDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Heavy lset: {} (lsets: {})", result.summary(), result.lsets);
    assert!(result.is_success(), "Heavy lset should maintain invariants");
}

// =============================================================================
// Mixed Configuration Tests
// =============================================================================

#[test]
fn test_list_dst_50_seeds_mixed_configs() {
    let mut all_passed = true;
    let mut failures = Vec::new();

    for seed in 0..50 {
        let config = match seed % 3 {
            0 => ListDSTConfig::new(seed),
            1 => ListDSTConfig::high_churn(seed),
            _ => ListDSTConfig::modify_heavy(seed),
        };

        let mut harness = ListDSTHarness::new(config);
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
// Longer Tests (ignored by default)
// =============================================================================

#[test]
#[ignore]
fn test_list_dst_500_seeds() {
    let results = run_list_batch(0, 500, 500, ListDSTConfig::new);
    let summary = summarize_list_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 500, "All 500 seeds should pass");
}

#[test]
#[ignore]
fn test_list_dst_stress_10000_ops() {
    let mut harness = ListDSTHarness::with_seed(31415);
    harness.run(10000);
    let result = harness.result();
    println!("Stress 10000 ops: {}", result.summary());
    assert!(result.is_success(), "10000 ops should maintain invariants");
}
