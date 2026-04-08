//! DST Batch Verification Test
//!
//! Runs 1000 seeds with varying fault configurations to verify:
//! - Determinism across all seeds
//! - Crash/recovery handling
//! - Fault tolerance

use redis_sim::buggify::{self, FaultConfig};
use redis_sim::simulator::dst::{BatchRunner, DSTConfig};
use redis_sim::simulator::dst_integration::run_redis_dst_batch;

#[test]
fn test_1000_seeds_calm() {
    let stats = run_redis_dst_batch(10000, 1000, 100, FaultConfig::calm());

    println!("\n=== 1000 Seeds (Calm) ===");
    println!("{}", stats.summary());

    assert!(stats.all_passed(), "Failed seeds: {:?}", stats.failed_seeds);
    assert!(
        stats.total_operations > 100_000,
        "Expected >100K ops, got {}",
        stats.total_operations
    );
}

#[test]
fn test_500_seeds_moderate() {
    let stats = run_redis_dst_batch(20000, 500, 100, FaultConfig::moderate());

    println!("\n=== 500 Seeds (Moderate) ===");
    println!("{}", stats.summary());

    assert!(stats.all_passed(), "Failed seeds: {:?}", stats.failed_seeds);
    assert!(
        stats.total_crashes > 0,
        "Expected some crashes with moderate faults"
    );
}

#[test]
fn test_100_seeds_chaos() {
    let stats = run_redis_dst_batch(30000, 100, 200, FaultConfig::chaos());

    println!("\n=== 100 Seeds (Chaos) ===");
    println!("{}", stats.summary());

    assert!(stats.all_passed(), "Failed seeds: {:?}", stats.failed_seeds);
    assert!(
        stats.total_crashes > 10,
        "Expected significant crashes with chaos"
    );
}

#[test]
fn test_determinism_verification() {
    // Run same seed twice with same config
    let seed = 42424242;

    buggify::reset_stats();
    let stats1 = run_redis_dst_batch(seed, 1, 100, FaultConfig::moderate());

    buggify::reset_stats();
    let stats2 = run_redis_dst_batch(seed, 1, 100, FaultConfig::moderate());

    assert_eq!(
        stats1.total_operations, stats2.total_operations,
        "Operations differ: {} vs {}",
        stats1.total_operations, stats2.total_operations
    );
    assert_eq!(
        stats1.total_crashes, stats2.total_crashes,
        "Crashes differ: {} vs {}",
        stats1.total_crashes, stats2.total_crashes
    );

    println!("\n=== Determinism Verified ===");
    println!(
        "Operations: {}, Crashes: {}",
        stats1.total_operations, stats1.total_crashes
    );
}

#[test]
fn test_batch_runner_core_dst() {
    let config = DSTConfig {
        fault_config: FaultConfig::moderate(),
        ..Default::default()
    };
    let runner = BatchRunner::new(50000, 1000).with_config(config);
    let results = runner.run_default(100);

    println!("\n=== Core DST Batch (1000 seeds) ===");
    println!("Total runs: {}", results.total_runs);
    println!("Total operations: {}", results.total_operations);
    println!("Total crashes: {}", results.total_crashes);
    println!("Total recoveries: {}", results.total_recoveries);
    println!("Failures: {}", results.failed_seeds.len());

    assert!(
        results.failed_seeds.is_empty(),
        "Failed seeds: {:?}",
        results.failed_seeds
    );
    assert!(results.total_operations >= 100_000);
}

/// Test using BUGGIFY_CONFIG env var for fault configuration.
/// This is the entry point for the GEPA DST optimizer.
///
/// Run with: BUGGIFY_CONFIG="global_multiplier=2.0,..." cargo test --release --test dst_batch_verification test_env_config
#[test]
fn test_env_config_batch() {
    let fault_config = FaultConfig::from_env_or_default();
    let gm = fault_config.global_multiplier;

    let stats = run_redis_dst_batch(40000, 50, 200, fault_config);

    // Structured output for GEPA optimizer parsing
    println!("\n=== GEPA DST Evaluation ===");
    println!("GEPA_GLOBAL_MULTIPLIER={:.4}", gm);
    println!("GEPA_TOTAL_RUNS={}", stats.total_runs);
    println!("GEPA_TOTAL_OPS={}", stats.total_operations);
    println!("GEPA_TOTAL_CRASHES={}", stats.total_crashes);
    println!("GEPA_TOTAL_RECOVERIES={}", stats.total_recoveries);
    println!("GEPA_FAILURES={}", stats.failed_seeds.len());
    println!("GEPA_FAILED_SEEDS={:?}", stats.failed_seeds);
    println!("{}", stats.summary());

    // Report buggify stats for coverage tracking
    let bstats = buggify::get_stats();
    let total_checks: u64 = bstats.checks.values().sum();
    let total_triggers: u64 = bstats.triggers.values().sum();
    let faults_with_triggers = bstats.triggers.len();
    println!("GEPA_BUGGIFY_CHECKS={}", total_checks);
    println!("GEPA_BUGGIFY_TRIGGERS={}", total_triggers);
    println!("GEPA_FAULTS_TRIGGERED={}", faults_with_triggers);

    // This test always passes — the optimizer reads the structured output
    // to compute fitness. A "failure" in DST means a bug was found, which
    // is what the optimizer is trying to maximize.
}
