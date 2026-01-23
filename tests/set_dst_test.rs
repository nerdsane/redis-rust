//! Set Deterministic Simulation Tests
//!
//! VOPR-style tests for RedisSet with multiple seeds.

use redis_sim::redis::{run_set_batch, summarize_set_batch, SetDSTConfig, SetDSTHarness};

// =============================================================================
// Standard Configuration Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_set_dst_100_seeds_standard() {
    let results = run_set_batch(0, 100, 500, SetDSTConfig::new);
    let summary = summarize_set_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed, 100,
        "All 100 seeds should pass with standard config"
    );
}

#[test]
fn test_set_dst_100_seeds_small_members() {
    let results = run_set_batch(1000, 100, 500, SetDSTConfig::small_members);
    let summary = summarize_set_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with small members");
}

#[test]
fn test_set_dst_100_seeds_high_churn() {
    let results = run_set_batch(2000, 100, 500, SetDSTConfig::high_churn);
    let summary = summarize_set_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with high churn");
}

#[test]
fn test_set_dst_100_seeds_large_members() {
    let results = run_set_batch(3000, 100, 500, SetDSTConfig::large_members);
    let summary = summarize_set_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All 100 seeds should pass with large members");
}

// =============================================================================
// Stress Tests
// =============================================================================

#[test]
fn test_set_dst_stress_1000_ops() {
    let mut harness = SetDSTHarness::with_seed(42);
    harness.run(1000);
    let result = harness.result();
    println!("Stress 1000 ops: {}", result.summary());
    assert!(result.is_success(), "1000 ops should maintain invariants");
}

#[test]
fn test_set_dst_stress_5000_ops() {
    let mut harness = SetDSTHarness::with_seed(12345);
    harness.run(5000);
    let result = harness.result();
    println!("Stress 5000 ops: {}", result.summary());
    assert!(result.is_success(), "5000 ops should maintain invariants");
}

#[test]
fn test_set_dst_stress_small_members_2000_ops() {
    let config = SetDSTConfig::small_members(99999);
    let mut harness = SetDSTHarness::new(config);
    harness.run(2000);
    let result = harness.result();
    println!(
        "Stress small members 2000 ops: {} (add_existed:{}, remove_not_found:{})",
        result.summary(),
        result.add_existed,
        result.remove_not_found
    );
    assert!(
        result.is_success(),
        "Small members stress should maintain invariants"
    );
    // Verify we exercised collision paths
    assert!(
        result.add_existed > 100,
        "Should have many add collisions with small member space"
    );
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_set_dst_high_remove_rate() {
    // Configuration with very high remove probability
    let config = SetDSTConfig {
        seed: 77777,
        num_members: 30,
        remove_prob: 0.6,
    };

    let mut harness = SetDSTHarness::new(config);
    harness.run(1000);
    let result = harness.result();
    println!("High remove rate: {}", result.summary());
    assert!(
        result.is_success(),
        "High remove rate should maintain invariants"
    );
}

#[test]
fn test_set_dst_tiny_member_space() {
    // Very small member space = constant overwrites
    let config = SetDSTConfig {
        seed: 88888,
        num_members: 3, // Only 3 members!
        remove_prob: 0.3,
    };

    let mut harness = SetDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Tiny member space (3 members): {}", result.summary());
    assert!(
        result.is_success(),
        "Tiny member space should maintain invariants"
    );
}

#[test]
fn test_set_dst_mostly_adds() {
    // Configuration that favors adds
    let config = SetDSTConfig {
        seed: 11111,
        num_members: 200,
        remove_prob: 0.05,
    };

    let mut harness = SetDSTHarness::new(config);
    harness.run(500);
    let result = harness.result();
    println!("Mostly adds: {} (adds:{})", result.summary(), result.adds);
    assert!(
        result.is_success(),
        "Mostly adds should maintain invariants"
    );
}

#[test]
fn test_set_dst_balanced() {
    // Balanced add/remove
    let config = SetDSTConfig {
        seed: 22222,
        num_members: 50,
        remove_prob: 0.5,
    };

    let mut harness = SetDSTHarness::new(config);
    harness.run(1000);
    let result = harness.result();
    println!("Balanced: {}", result.summary());
    assert!(
        result.is_success(),
        "Balanced operations should maintain invariants"
    );
}

// =============================================================================
// Mixed Configuration Tests
// =============================================================================

#[test]
fn test_set_dst_50_seeds_mixed_configs() {
    let mut all_passed = true;
    let mut failures = Vec::new();

    for seed in 0..50 {
        let config = match seed % 4 {
            0 => SetDSTConfig::new(seed),
            1 => SetDSTConfig::small_members(seed),
            2 => SetDSTConfig::high_churn(seed),
            _ => SetDSTConfig::large_members(seed),
        };

        let mut harness = SetDSTHarness::new(config);
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
fn test_set_dst_500_seeds() {
    let results = run_set_batch(0, 500, 500, SetDSTConfig::new);
    let summary = summarize_set_batch(&results);
    println!("{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 500, "All 500 seeds should pass");
}

#[test]
#[ignore]
fn test_set_dst_stress_10000_ops() {
    let mut harness = SetDSTHarness::with_seed(31415);
    harness.run(10000);
    let result = harness.result();
    println!("Stress 10000 ops: {}", result.summary());
    assert!(result.is_success(), "10000 ops should maintain invariants");
}
