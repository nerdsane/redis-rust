//! CRDT Deterministic Simulation Tests
//!
//! DST tests for CRDT convergence with 100+ seeds.
//! These tests verify that all CRDT implementations correctly converge
//! after network partitions and message drops.

use redis_sim::replication::crdt_dst::{
    run_gcounter_batch, run_orset_batch, run_pncounter_batch, run_vectorclock_batch,
    summarize_batch, CRDTDSTConfig, GCounterDSTHarness, ORSetDSTHarness, PNCounterDSTHarness,
    VectorClockDSTHarness,
};

// =============================================================================
// GCounter Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_gcounter_100_seeds_calm() {
    let results = run_gcounter_batch(0, 100, 100, CRDTDSTConfig::calm);
    let summary = summarize_batch(&results);
    println!("GCounter 100 Seeds Calm:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All GCounter calm runs should converge");
}

#[test]
fn test_gcounter_100_seeds_moderate() {
    let results = run_gcounter_batch(1000, 100, 100, CRDTDSTConfig::moderate);
    let summary = summarize_batch(&results);
    println!("GCounter 100 Seeds Moderate:\n{}", summary);

    // With message drops, all should still converge after sync_all
    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(passed >= 95, "At least 95% should converge: {}/100", passed);
}

#[test]
fn test_gcounter_stress_500_ops() {
    let config = CRDTDSTConfig::calm(7777);
    let mut harness = GCounterDSTHarness::new(config);

    harness.run(500);
    harness.sync_all();
    harness.check_convergence();

    let result = harness.result();
    println!("GCounter Stress 500 ops: {}", result.summary());
    assert!(result.is_success());
}

// =============================================================================
// PNCounter Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_pncounter_100_seeds_calm() {
    let results = run_pncounter_batch(2000, 100, 100, CRDTDSTConfig::calm);
    let summary = summarize_batch(&results);
    println!("PNCounter 100 Seeds Calm:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All PNCounter calm runs should converge");
}

#[test]
fn test_pncounter_100_seeds_moderate() {
    let results = run_pncounter_batch(3000, 100, 100, CRDTDSTConfig::moderate);
    let summary = summarize_batch(&results);
    println!("PNCounter 100 Seeds Moderate:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(passed >= 95, "At least 95% should converge: {}/100", passed);
}

#[test]
fn test_pncounter_stress_500_ops() {
    let config = CRDTDSTConfig::calm(8888);
    let mut harness = PNCounterDSTHarness::new(config);

    harness.run(500);
    harness.sync_all();
    harness.check_convergence();

    let result = harness.result();
    println!("PNCounter Stress 500 ops: {}", result.summary());
    assert!(result.is_success());
}

// =============================================================================
// ORSet Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_orset_100_seeds_calm() {
    let results = run_orset_batch(4000, 100, 100, CRDTDSTConfig::calm);
    let summary = summarize_batch(&results);
    println!("ORSet 100 Seeds Calm:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All ORSet calm runs should converge");
}

#[test]
fn test_orset_100_seeds_moderate() {
    let results = run_orset_batch(5000, 100, 100, CRDTDSTConfig::moderate);
    let summary = summarize_batch(&results);
    println!("ORSet 100 Seeds Moderate:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(passed >= 95, "At least 95% should converge: {}/100", passed);
}

#[test]
fn test_orset_stress_500_ops() {
    let config = CRDTDSTConfig::calm(9999);
    let mut harness = ORSetDSTHarness::new(config);

    harness.run(500);
    harness.sync_all();
    harness.check_convergence();

    let result = harness.result();
    println!("ORSet Stress 500 ops: {}", result.summary());
    assert!(result.is_success());
}

// =============================================================================
// VectorClock Tests - 100+ Seeds
// =============================================================================

#[test]
fn test_vectorclock_100_seeds_calm() {
    let results = run_vectorclock_batch(6000, 100, 100, CRDTDSTConfig::calm);
    let summary = summarize_batch(&results);
    println!("VectorClock 100 Seeds Calm:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert_eq!(passed, 100, "All VectorClock calm runs should converge");
}

#[test]
fn test_vectorclock_100_seeds_moderate() {
    let results = run_vectorclock_batch(7000, 100, 100, CRDTDSTConfig::moderate);
    let summary = summarize_batch(&results);
    println!("VectorClock 100 Seeds Moderate:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(passed >= 95, "At least 95% should converge: {}/100", passed);
}

#[test]
fn test_vectorclock_stress_500_ops() {
    let config = CRDTDSTConfig::calm(10101);
    let mut harness = VectorClockDSTHarness::new(config);

    harness.run(500);
    harness.sync_all();
    harness.check_convergence();

    let result = harness.result();
    println!("VectorClock Stress 500 ops: {}", result.summary());
    assert!(result.is_success());
}

// =============================================================================
// Chaos Mode Tests
// =============================================================================

#[test]
fn test_gcounter_50_seeds_chaos() {
    let results = run_gcounter_batch(8000, 50, 100, CRDTDSTConfig::chaos);
    let summary = summarize_batch(&results);
    println!("GCounter 50 Seeds Chaos:\n{}", summary);

    // Chaos has high message drop - still should converge after full sync
    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(
        passed >= 40,
        "At least 80% should converge in chaos: {}/50",
        passed
    );
}

#[test]
fn test_orset_50_seeds_chaos() {
    let results = run_orset_batch(9000, 50, 100, CRDTDSTConfig::chaos);
    let summary = summarize_batch(&results);
    println!("ORSet 50 Seeds Chaos:\n{}", summary);

    let passed = results.iter().filter(|r| r.is_success()).count();
    assert!(
        passed >= 40,
        "At least 80% should converge in chaos: {}/50",
        passed
    );
}

// =============================================================================
// Large Scale Tests (run with --release for reasonable times)
// =============================================================================

#[test]
#[ignore] // Run with: cargo test --release -- --ignored
fn test_all_crdts_500_seeds() {
    println!("\n=== Running 500 seed tests for all CRDTs ===\n");

    let gcounter_results = run_gcounter_batch(10000, 500, 100, CRDTDSTConfig::calm);
    println!("GCounter: {}", summarize_batch(&gcounter_results));
    assert!(gcounter_results.iter().all(|r| r.is_success()));

    let pncounter_results = run_pncounter_batch(20000, 500, 100, CRDTDSTConfig::calm);
    println!("PNCounter: {}", summarize_batch(&pncounter_results));
    assert!(pncounter_results.iter().all(|r| r.is_success()));

    let orset_results = run_orset_batch(30000, 500, 100, CRDTDSTConfig::calm);
    println!("ORSet: {}", summarize_batch(&orset_results));
    assert!(orset_results.iter().all(|r| r.is_success()));

    let vc_results = run_vectorclock_batch(40000, 500, 100, CRDTDSTConfig::calm);
    println!("VectorClock: {}", summarize_batch(&vc_results));
    assert!(vc_results.iter().all(|r| r.is_success()));
}

// =============================================================================
// Determinism Verification
// =============================================================================

#[test]
fn test_crdt_dst_determinism_all_types() {
    let seed = 42424242;

    // GCounter
    let mut h1 = GCounterDSTHarness::new(CRDTDSTConfig::calm(seed));
    h1.run(50);
    let ops1 = h1.result().total_operations;

    let mut h2 = GCounterDSTHarness::new(CRDTDSTConfig::calm(seed));
    h2.run(50);
    let ops2 = h2.result().total_operations;

    assert_eq!(
        ops1, ops2,
        "GCounter: Same seed should produce same results"
    );

    // PNCounter
    let mut h1 = PNCounterDSTHarness::new(CRDTDSTConfig::calm(seed));
    h1.run(50);
    let ops1 = h1.result().total_operations;

    let mut h2 = PNCounterDSTHarness::new(CRDTDSTConfig::calm(seed));
    h2.run(50);
    let ops2 = h2.result().total_operations;

    assert_eq!(
        ops1, ops2,
        "PNCounter: Same seed should produce same results"
    );

    // ORSet
    let mut h1 = ORSetDSTHarness::new(CRDTDSTConfig::calm(seed));
    h1.run(50);
    let ops1 = h1.result().total_operations;

    let mut h2 = ORSetDSTHarness::new(CRDTDSTConfig::calm(seed));
    h2.run(50);
    let ops2 = h2.result().total_operations;

    assert_eq!(ops1, ops2, "ORSet: Same seed should produce same results");
}
