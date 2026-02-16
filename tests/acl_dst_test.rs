//! ACL Deterministic Simulation Tests
//!
//! DST tests for the ACL system with multiple seeds.
//! These tests verify that the AclManager matches a shadow specification
//! for randomly generated sequences of ACL operations.

#![cfg(feature = "acl")]

use redis_sim::security::acl_dst::{
    run_acl_batch, summarize_acl_batch, AclDSTConfig, AclDSTHarness,
};

// =============================================================================
// Standard Configuration Tests - 100 Seeds
// =============================================================================

#[test]
fn test_acl_dst_100_seeds_standard() {
    let results = run_acl_batch(0, 100, 500, AclDSTConfig::new);
    let summary = summarize_acl_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed, 100,
        "All 100 seeds should pass with standard config"
    );
}

#[test]
fn test_acl_dst_100_seeds_small_users() {
    let results = run_acl_batch(1000, 100, 500, AclDSTConfig::small_users);
    let summary = summarize_acl_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with small users");
}

#[test]
fn test_acl_dst_100_seeds_large_users() {
    let results = run_acl_batch(2000, 100, 500, AclDSTConfig::large_users);
    let summary = summarize_acl_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with large users");
}

#[test]
fn test_acl_dst_100_seeds_high_churn() {
    let results = run_acl_batch(3000, 100, 1000, AclDSTConfig::high_churn);
    let summary = summarize_acl_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with high churn");
}

// =============================================================================
// Stress Tests
// =============================================================================

#[test]
fn test_acl_dst_stress_2000_ops() {
    let mut harness = AclDSTHarness::with_seed(42);
    harness.run(2000);
    let result = harness.result();
    println!("Stress 2000 ops: {}", result.summary());
    assert!(
        result.is_success(),
        "2000 ops should maintain invariants: {:?}",
        result.invariant_violations
    );
}

#[test]
fn test_acl_dst_stress_5000_ops() {
    let config = AclDSTConfig::high_churn(12345);
    let mut harness = AclDSTHarness::new(config);
    harness.run(5000);
    let result = harness.result();
    println!("Stress 5000 ops: {}", result.summary());
    assert!(
        result.is_success(),
        "5000 ops should maintain invariants: {:?}",
        result.invariant_violations
    );
}

// =============================================================================
// Mixed Configuration Tests
// =============================================================================

#[test]
fn test_acl_dst_50_seeds_mixed_configs() {
    let mut all_passed = true;
    let mut failures = Vec::new();

    for seed in 0..50 {
        let config = match seed % 4 {
            0 => AclDSTConfig::new(seed),
            1 => AclDSTConfig::small_users(seed),
            2 => AclDSTConfig::high_churn(seed),
            _ => AclDSTConfig::large_users(seed),
        };

        let mut harness = AclDSTHarness::new(config);
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
fn test_acl_dst_500_seeds() {
    let results = run_acl_batch(0, 500, 500, AclDSTConfig::new);
    let summary = summarize_acl_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 500, "All 500 seeds should pass");
}

#[test]
#[ignore]
fn test_acl_dst_stress_10000_ops() {
    let config = AclDSTConfig::high_churn(31415);
    let mut harness = AclDSTHarness::new(config);
    harness.run(10000);
    let result = harness.result();
    println!("Stress 10000 ops: {}", result.summary());
    assert!(
        result.is_success(),
        "10000 ops should maintain invariants: {:?}",
        result.invariant_violations
    );
}
