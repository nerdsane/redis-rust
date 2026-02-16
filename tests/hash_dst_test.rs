//! Hash Deterministic Simulation Tests
//!
//! DST tests for RedisHash with multiple seeds.

use redis_sim::redis::{run_hash_batch, summarize_hash_batch, HashDSTConfig, HashDSTHarness};

// =============================================================================
// Standard Configuration Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_hash_dst_100_seeds_standard() {
    let results = run_hash_batch(0, 100, 500, HashDSTConfig::new);
    let summary = summarize_hash_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed, 100,
        "All 100 seeds should pass with standard config"
    );
}

#[test]
fn test_hash_dst_100_seeds_small_fields() {
    let results = run_hash_batch(1000, 100, 500, HashDSTConfig::small_fields);
    let summary = summarize_hash_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with small fields");
}

#[test]
fn test_hash_dst_100_seeds_high_churn() {
    let results = run_hash_batch(2000, 100, 500, HashDSTConfig::high_churn);
    let summary = summarize_hash_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with high churn");
}

// =============================================================================
// Stress Tests
// =============================================================================

#[test]
fn test_hash_dst_stress_1000_ops() {
    let mut harness = HashDSTHarness::with_seed(42);
    harness.run(1000);
    let result = harness.result();
    println!("Stress 1000 ops: {}", result.summary());
    assert!(result.is_success(), "1000 ops should maintain invariants");
}

#[test]
fn test_hash_dst_stress_5000_ops() {
    let mut harness = HashDSTHarness::with_seed(12345);
    harness.run(5000);
    let result = harness.result();
    println!("Stress 5000 ops: {}", result.summary());
    assert!(result.is_success(), "5000 ops should maintain invariants");
}

#[test]
fn test_hash_dst_stress_small_fields_2000_ops() {
    let config = HashDSTConfig::small_fields(99999);
    let mut harness = HashDSTHarness::new(config);
    harness.run(2000);
    let result = harness.result();
    println!(
        "Stress small fields 2000 ops: {} (updates:{}, deletes:{})",
        result.summary(),
        result.updates,
        result.deletes
    );
    assert!(
        result.is_success(),
        "Small fields stress should maintain invariants"
    );
    // Verify we exercised updates
    assert!(
        result.updates > 100,
        "Should have many updates with small field space"
    );
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_hash_dst_high_delete_rate() {
    // Configuration with very high delete probability
    let config = HashDSTConfig {
        seed: 77777,
        num_fields: 30,
        num_values: 20,
        delete_prob: 0.5,
        update_prob: 0.2,
    };

    let mut harness = HashDSTHarness::new(config);
    harness.run(1000);
    let result = harness.result();
    println!("High delete rate: {}", result.summary());
    assert!(
        result.is_success(),
        "High delete rate should maintain invariants"
    );
}

#[test]
fn test_hash_dst_tiny_field_space() {
    // Very small field space = constant overwrites
    let config = HashDSTConfig {
        seed: 88888,
        num_fields: 3, // Only 3 fields!
        num_values: 10,
        delete_prob: 0.2,
        update_prob: 0.6,
    };

    let mut harness = HashDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Tiny field space (3 fields): {}", result.summary());
    assert!(
        result.is_success(),
        "Tiny field space should maintain invariants"
    );
}

#[test]
fn test_hash_dst_mostly_updates() {
    // Configuration that favors updates over new sets
    let config = HashDSTConfig {
        seed: 11111,
        num_fields: 20,
        num_values: 100,
        delete_prob: 0.1,
        update_prob: 0.7,
    };

    let mut harness = HashDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!(
        "Mostly updates: {} (updates:{})",
        result.summary(),
        result.updates
    );
    assert!(
        result.is_success(),
        "Mostly updates should maintain invariants"
    );
}

#[test]
fn test_hash_dst_large_field_space() {
    // Large field space = mostly new sets
    let config = HashDSTConfig {
        seed: 22222,
        num_fields: 500,
        num_values: 50,
        delete_prob: 0.1,
        update_prob: 0.1,
    };

    let mut harness = HashDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Large field space: {}", result.summary());
    assert!(
        result.is_success(),
        "Large field space should maintain invariants"
    );
}

// =============================================================================
// Mixed Configuration Tests
// =============================================================================

#[test]
fn test_hash_dst_50_seeds_mixed_configs() {
    let mut all_passed = true;
    let mut failures = Vec::new();

    for seed in 0..50 {
        let config = match seed % 3 {
            0 => HashDSTConfig::new(seed),
            1 => HashDSTConfig::small_fields(seed),
            _ => HashDSTConfig::high_churn(seed),
        };

        let mut harness = HashDSTHarness::new(config);
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
fn test_hash_dst_500_seeds() {
    let results = run_hash_batch(0, 500, 500, HashDSTConfig::new);
    let summary = summarize_hash_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 500, "All 500 seeds should pass");
}

#[test]
#[ignore]
fn test_hash_dst_stress_10000_ops() {
    let mut harness = HashDSTHarness::with_seed(31415);
    harness.run(10000);
    let result = harness.result();
    println!("Stress 10000 ops: {}", result.summary());
    assert!(result.is_success(), "10000 ops should maintain invariants");
}
