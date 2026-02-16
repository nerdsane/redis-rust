//! Sorted Set Deterministic Simulation Tests
//!
//! VOPR-style tests for RedisSortedSet with multiple seeds.
//! These tests verify that sorted set operations maintain invariants
//! under various random operation sequences.

use redis_sim::redis::{
    run_sorted_set_batch, summarize_batch, SortedSetDSTConfig, SortedSetDSTHarness,
};

// =============================================================================
// Standard Configuration Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_sorted_set_dst_100_seeds_standard() {
    let results = run_sorted_set_batch(0, 100, 500, SortedSetDSTConfig::new);
    let summary = summarize_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed, 100,
        "All 100 seeds should pass with standard config"
    );
}

#[test]
fn test_sorted_set_dst_100_seeds_small_keyspace() {
    // Small keyspace = more collisions, updates, and removes
    let results = run_sorted_set_batch(1000, 100, 500, SortedSetDSTConfig::small_keyspace);
    let summary = summarize_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with small keyspace");
}

#[test]
fn test_sorted_set_dst_100_seeds_large_keyspace() {
    // Large keyspace = mostly adds, fewer collisions
    let results = run_sorted_set_batch(2000, 100, 500, SortedSetDSTConfig::large_keyspace);
    let summary = summarize_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with large keyspace");
}

// =============================================================================
// Stress Tests - High Operation Count
// =============================================================================

#[test]
fn test_sorted_set_dst_stress_1000_ops() {
    let mut harness = SortedSetDSTHarness::with_seed(42);
    harness.run(1000);
    let result = harness.result();
    println!("Stress 1000 ops: {}", result.summary());
    assert!(result.is_success(), "1000 ops should maintain invariants");
}

#[test]
fn test_sorted_set_dst_stress_5000_ops() {
    let mut harness = SortedSetDSTHarness::with_seed(12345);
    harness.run(5000);
    let result = harness.result();
    println!("Stress 5000 ops: {}", result.summary());
    assert!(result.is_success(), "5000 ops should maintain invariants");
}

#[test]
fn test_sorted_set_dst_stress_small_keyspace_2000_ops() {
    // Many operations on small key space = lots of updates and removes
    let config = SortedSetDSTConfig::small_keyspace(99999);
    let mut harness = SortedSetDSTHarness::new(config);
    harness.run(2000);
    let result = harness.result();
    println!(
        "Stress small keyspace 2000 ops: {} (updates: {}, removes: {})",
        result.summary(),
        result.updates,
        result.removes
    );
    assert!(
        result.is_success(),
        "Small keyspace stress should maintain invariants"
    );
    // Verify we actually exercised updates and removes
    assert!(result.updates > 100, "Should have many updates");
    assert!(result.removes > 50, "Should have many removes");
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_sorted_set_dst_high_remove_rate() {
    // Configuration with high remove probability
    let config = SortedSetDSTConfig {
        seed: 77777,
        num_keys: 50,
        update_prob: 0.2,
        remove_prob: 0.4, // 40% removes
        max_score: 100.0,
    };

    let mut harness = SortedSetDSTHarness::new(config);
    harness.run(1000);
    let result = harness.result();
    println!("High remove rate: {}", result.summary());
    assert!(
        result.is_success(),
        "High remove rate should maintain invariants"
    );
}

#[test]
fn test_sorted_set_dst_tiny_keyspace() {
    // Very small keyspace = constant overwrites
    let config = SortedSetDSTConfig {
        seed: 88888,
        num_keys: 3, // Only 3 keys!
        update_prob: 0.5,
        remove_prob: 0.3,
        max_score: 10.0,
    };

    let mut harness = SortedSetDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Tiny keyspace (3 keys): {}", result.summary());
    assert!(
        result.is_success(),
        "Tiny keyspace should maintain invariants"
    );
}

#[test]
fn test_sorted_set_dst_score_precision() {
    // Test with very precise scores (many decimal places)
    let config = SortedSetDSTConfig {
        seed: 11111,
        num_keys: 100,
        update_prob: 0.4,
        remove_prob: 0.1,
        max_score: 1.0, // Scores between 0.00 and 1.00
    };

    let mut harness = SortedSetDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Score precision test: {}", result.summary());
    assert!(
        result.is_success(),
        "Precise scores should maintain invariants"
    );
}

// =============================================================================
// Batch Tests for CI
// =============================================================================

#[test]
fn test_sorted_set_dst_50_seeds_mixed_configs() {
    // Run 50 seeds with different configurations
    let mut all_passed = true;
    let mut failures = Vec::new();

    for seed in 0..50 {
        let config = match seed % 3 {
            0 => SortedSetDSTConfig::new(seed),
            1 => SortedSetDSTConfig::small_keyspace(seed),
            _ => SortedSetDSTConfig::large_keyspace(seed),
        };

        let mut harness = SortedSetDSTHarness::new(config);
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
fn test_sorted_set_dst_500_seeds() {
    let results = run_sorted_set_batch(0, 500, 500, SortedSetDSTConfig::new);
    let summary = summarize_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 500, "All 500 seeds should pass");
}

#[test]
#[ignore]
fn test_sorted_set_dst_stress_10000_ops() {
    let mut harness = SortedSetDSTHarness::with_seed(31415);
    harness.run(10000);
    let result = harness.result();
    println!("Stress 10000 ops: {}", result.summary());
    assert!(result.is_success(), "10000 ops should maintain invariants");
}
