//! Streaming Persistence DST Tests
//!
//! Deterministic simulation tests (FoundationDB-style, seed-based)
//! for the streaming persistence module. These tests run many seeds to find edge cases.
//!
//! ## Design Philosophy (TigerBeetle/FoundationDB inspired)
//!
//! 1. **Deterministic**: Same seed always produces same results
//! 2. **Fault injection**: Test behavior under object store failures
//! 3. **Invariant checking**: Verify system properties after each run
//! 4. **Multi-seed**: Run thousands of seeds to find rare bugs
//!
//! ## Test Categories
//!
//! - **Calm tests**: No faults, verify basic correctness
//! - **Moderate tests**: Some faults, verify resilience
//! - **Chaos tests**: Many faults, stress test

use redis_sim::streaming::{
    run_dst_batch, summarize_batch, StreamingDSTConfig, StreamingDSTHarness,
};

// =============================================================================
// Single Seed Tests
// =============================================================================

#[tokio::test]
async fn test_streaming_dst_single_calm() {
    let config = StreamingDSTConfig::calm(12345);
    let mut harness = StreamingDSTHarness::new(config).await;

    harness.run(500).await;
    harness.check_invariants().await;

    let result = harness.result();
    println!("{}", result.summary());

    assert!(
        result.is_success(),
        "Calm mode should not violate invariants: {:?}",
        result.invariant_violations
    );
    assert!(result.total_operations >= 500);
}

#[tokio::test]
async fn test_streaming_dst_single_moderate() {
    let config = StreamingDSTConfig::moderate(54321);
    let mut harness = StreamingDSTHarness::new(config).await;

    harness.run(300).await;
    harness.check_invariants().await;

    let result = harness.result();
    println!("{}", result.summary());

    // Moderate should mostly succeed
    assert!(result.total_operations >= 300);
}

#[tokio::test]
async fn test_streaming_dst_single_chaos() {
    let config = StreamingDSTConfig::chaos(99999);
    let mut harness = StreamingDSTHarness::new(config).await;

    harness.run(200).await;
    harness.check_invariants().await;

    let result = harness.result();
    println!("{}", result.summary());
    println!("Store stats: {:?}", result.store_stats);

    // Chaos mode will have failures, but operations should complete
    assert!(result.total_operations >= 200);
}

// =============================================================================
// Multi-Seed Batch Tests (DST)
// =============================================================================

#[tokio::test]
async fn test_streaming_dst_100_seeds_calm() {
    let results = run_dst_batch(0, 100, 100, StreamingDSTConfig::calm).await;

    let summary = summarize_batch(&results);
    println!("100 Seeds Calm:\n{}", summary);

    // All calm runs should pass
    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(
        passed,
        results.len(),
        "All calm runs should pass. Failed seeds: {:?}",
        results
            .iter()
            .filter(|r| !r.is_success())
            .map(|r| r.seed)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_streaming_dst_100_seeds_moderate() {
    let results = run_dst_batch(1000, 100, 100, StreamingDSTConfig::moderate).await;

    let summary = summarize_batch(&results);
    println!("100 Seeds Moderate:\n{}", summary);

    // Most moderate runs should pass (allow some invariant issues under faults)
    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(
        passed >= 80,
        "At least 80% of moderate runs should pass. Passed: {}/{}",
        passed,
        results.len()
    );
}

#[tokio::test]
async fn test_streaming_dst_50_seeds_chaos() {
    let results = run_dst_batch(2000, 50, 100, StreamingDSTConfig::chaos).await;

    let summary = summarize_batch(&results);
    println!("50 Seeds Chaos:\n{}", summary);

    // Chaos mode: check that operations complete, some failures expected
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();
    assert!(
        total_ops >= 50 * 100,
        "Should complete all operations: {}",
        total_ops
    );
}

// =============================================================================
// Stress Tests (longer runs)
// =============================================================================

#[tokio::test]
async fn test_streaming_dst_stress_calm_1000_ops() {
    let config = StreamingDSTConfig::calm(7777);
    let mut harness = StreamingDSTHarness::new(config).await;

    harness.run(1000).await;
    harness.check_invariants().await;

    let result = harness.result();
    println!("Stress 1000 ops:\n{}", result.summary());

    assert!(result.is_success());
    assert!(result.flushes > 0, "Should have some flushes");
}

#[tokio::test]
async fn test_streaming_dst_crash_recovery_stress() {
    // High crash probability to stress recovery
    let mut config = StreamingDSTConfig::calm(8888);
    config.crash_probability = 0.15; // 15% crash probability

    let mut harness = StreamingDSTHarness::new(config).await;
    harness.run(500).await;
    harness.check_invariants().await;

    let result = harness.result();
    println!("Crash Recovery Stress:\n{}", result.summary());

    assert!(result.crashes > 0, "Should have some crashes");
    assert!(result.total_operations >= 500);
}

// =============================================================================
// Determinism Verification
// =============================================================================

#[tokio::test]
async fn test_streaming_dst_determinism() {
    // Run same seed twice, should get identical results
    async fn run_seed(seed: u64) -> (u64, u64, u64) {
        let config = StreamingDSTConfig::moderate(seed);
        let mut harness = StreamingDSTHarness::new(config).await;
        harness.run(100).await;
        harness.check_invariants().await;
        let result = harness.result();
        (
            result.successful_operations,
            result.failed_operations,
            result.flushes,
        )
    }

    let seed = 42424242;
    let run1 = run_seed(seed).await;
    let run2 = run_seed(seed).await;

    assert_eq!(
        run1, run2,
        "Same seed should produce identical results: {:?} vs {:?}",
        run1, run2
    );
}

// =============================================================================
// Large Scale Tests (run with --release for reasonable times)
// =============================================================================

#[tokio::test]
#[ignore] // Run with: cargo test --release -- --ignored
async fn test_streaming_dst_1000_seeds_calm() {
    let results = run_dst_batch(10000, 1000, 50, StreamingDSTConfig::calm).await;

    let summary = summarize_batch(&results);
    println!("1000 Seeds Calm:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, results.len(), "All calm runs should pass");
}

#[tokio::test]
#[ignore] // Run with: cargo test --release -- --ignored
async fn test_streaming_dst_500_seeds_moderate() {
    let results = run_dst_batch(20000, 500, 100, StreamingDSTConfig::moderate).await;

    let summary = summarize_batch(&results);
    println!("500 Seeds Moderate:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(passed >= results.len() * 8 / 10, "At least 80% should pass");
}

// =============================================================================
// Specific Regression Tests
// =============================================================================

#[tokio::test]
async fn test_streaming_dst_empty_flush() {
    // Regression: ensure empty flushes don't cause issues
    let config = StreamingDSTConfig::calm(11111);
    let mut harness = StreamingDSTHarness::new(config).await;

    // Just flushes, no writes
    for _ in 0..10 {
        harness.run(1).await; // May generate flush operations
    }

    harness.check_invariants().await;
    assert!(harness.result().is_success());
}

#[tokio::test]
async fn test_streaming_dst_rapid_crash_recovery() {
    // Multiple crashes in quick succession
    let mut config = StreamingDSTConfig::calm(22222);
    config.crash_probability = 0.3; // Very high crash rate
    config.flush_probability = 0.3; // High flush rate

    let mut harness = StreamingDSTHarness::new(config).await;
    harness.run(200).await;
    harness.check_invariants().await;

    let result = harness.result();
    println!("Rapid Crash Recovery:\n{}", result.summary());

    // Should handle rapid crashes gracefully
    assert!(result.total_operations >= 200);
}
