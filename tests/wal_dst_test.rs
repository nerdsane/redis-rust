//! WAL DST Integration Tests
//!
//! Multi-seed deterministic simulation tests for WAL durability guarantees.
//! These run the full WAL pipeline with fault injection and crash simulation.

use redis_sim::streaming::wal_dst::{
    run_wal_dst_batch, summarize_wal_dst_batch, WalDSTConfig,
};
use redis_sim::streaming::wal_store::SimulatedWalStoreConfig;

#[test]
fn test_wal_dst_100_seeds_always_mode() {
    // INVARIANT: In Always mode, every acknowledged (fsync'd) write survives crash + recovery.
    let config = WalDSTConfig {
        num_writes: 100,
        max_file_size: 512,
        store_config: SimulatedWalStoreConfig::no_faults(),
        simulate_crash: true,
        fsync_after_write: true,
    };

    let results = run_wal_dst_batch(0..100, config);
    let summary = summarize_wal_dst_batch(&results);

    for r in &results {
        assert!(
            r.passed,
            "Seed {} failed: {}",
            r.seed,
            r.error_message.as_deref().unwrap_or("unknown")
        );
        assert_eq!(
            r.missing_after_recovery, 0,
            "Seed {}: {} acknowledged writes missing after recovery",
            r.seed, r.missing_after_recovery
        );
    }

    println!("{}", summary);
}

#[test]
fn test_wal_dst_50_seeds_with_faults() {
    // Same invariant as above, but with disk fault injection.
    // Writes that fail (append or fsync) are not acknowledged,
    // so they're allowed to be missing.
    let config = WalDSTConfig {
        num_writes: 100,
        max_file_size: 512,
        store_config: SimulatedWalStoreConfig::default(),
        simulate_crash: true,
        fsync_after_write: true,
    };

    let results = run_wal_dst_batch(0..50, config);
    let summary = summarize_wal_dst_batch(&results);

    for r in &results {
        assert!(
            r.passed,
            "Seed {} failed: {}",
            r.seed,
            r.error_message.as_deref().unwrap_or("unknown")
        );
    }

    println!("{}", summary);
}

#[test]
fn test_wal_dst_crash_during_group_commit() {
    // Simulates crash during group commit: some entries appended but not fsync'd.
    // Only fully-fsync'd entries should be acknowledged.
    let config = WalDSTConfig {
        num_writes: 50,
        max_file_size: 256,
        store_config: SimulatedWalStoreConfig::no_faults(),
        simulate_crash: true,
        fsync_after_write: true,
    };

    let results = run_wal_dst_batch(0..50, config);

    for r in &results {
        assert!(
            r.passed,
            "Seed {} failed: {}",
            r.seed,
            r.error_message.as_deref().unwrap_or("unknown")
        );
    }
}

#[test]
fn test_wal_dst_rotation_under_load() {
    // Tests recovery across multiple WAL files after rotation.
    let config = WalDSTConfig {
        num_writes: 200,
        max_file_size: 100, // Very small to force many rotations
        store_config: SimulatedWalStoreConfig::no_faults(),
        simulate_crash: true,
        fsync_after_write: true,
    };

    let results = run_wal_dst_batch(0..30, config);
    let summary = summarize_wal_dst_batch(&results);

    for r in &results {
        assert!(
            r.passed,
            "Seed {} failed: {}",
            r.seed,
            r.error_message.as_deref().unwrap_or("unknown")
        );
        assert!(
            r.acknowledged_writes > 0,
            "Seed {}: no acknowledged writes",
            r.seed
        );
    }

    println!("{}", summary);
}

#[test]
fn test_wal_dst_truncation_correctness() {
    // Verifies that truncation doesn't remove entries that haven't been streamed.
    // We write entries, truncate old ones, crash, and verify recent entries survive.
    use redis_sim::io::simulation::SimulatedRng;
    use redis_sim::redis::SDS;
    use redis_sim::replication::lattice::{LamportClock, ReplicaId};
    use redis_sim::replication::state::{ReplicatedValue, ReplicationDelta};
    use redis_sim::streaming::wal::{WalEntry, WalRotator};
    use redis_sim::streaming::wal_store::InMemoryWalStore;

    for seed in 0..30 {
        let store = InMemoryWalStore::new();
        let mut rotator = WalRotator::new(store.clone(), 100).unwrap();
        let mut rng = SimulatedRng::new(seed);

        let mut acked_after_truncation = Vec::new();

        // Phase 1: Write entries with timestamps 1-100
        for i in 1..=100u64 {
            let replica_id = ReplicaId::new(1);
            let clock = LamportClock {
                time: i,
                replica_id,
            };
            let replicated =
                ReplicatedValue::with_value(SDS::from_str(&format!("v{}", i)), clock);
            let delta = ReplicationDelta::new(format!("k{}", i), replicated, replica_id);
            let entry = WalEntry::from_delta(&delta, i).unwrap();
            rotator.append(&entry).unwrap();
            rotator.sync().unwrap();
        }

        // Phase 2: Truncate entries with timestamp <= 50 (simulate streaming caught up to ts=50)
        let _deleted = rotator.truncate_before(50).unwrap();

        // Phase 3: Write more entries with timestamps 101-150
        for i in 101..=150u64 {
            let replica_id = ReplicaId::new(1);
            let clock = LamportClock {
                time: i,
                replica_id,
            };
            let replicated =
                ReplicatedValue::with_value(SDS::from_str(&format!("v{}", i)), clock);
            let delta = ReplicationDelta::new(format!("k{}", i), replicated, replica_id);
            let entry = WalEntry::from_delta(&delta, i).unwrap();
            rotator.append(&entry).unwrap();
            rotator.sync().unwrap();
            acked_after_truncation.push(i);
        }

        // Phase 4: Recover and verify
        let recovery_rotator = WalRotator::new(store, 100).unwrap();
        let recovered = recovery_rotator.recover_all_entries().unwrap();
        let recovered_ts: std::collections::HashSet<u64> =
            recovered.iter().map(|e| e.timestamp).collect();

        // All entries written after truncation MUST be present
        for ts in &acked_after_truncation {
            assert!(
                recovered_ts.contains(ts),
                "Seed {}: entry with timestamp {} missing after truncation+recovery",
                seed,
                ts
            );
        }

        // Entries 51-100 should still be present: they're in files that also contain
        // entries > 50, so conservative truncation (whole-file granularity) keeps them.
        for ts in 51..=100u64 {
            assert!(
                recovered_ts.contains(&ts),
                "Seed {}: entry {} (51-100 range) missing — truncation was too aggressive",
                seed,
                ts
            );
        }
    }
}

#[test]
#[ignore] // Stress test — run manually with `cargo test --release -- --ignored`
fn test_wal_dst_1000_seeds_chaos() {
    let config = WalDSTConfig {
        num_writes: 200,
        max_file_size: 256,
        store_config: SimulatedWalStoreConfig::high_chaos(),
        simulate_crash: true,
        fsync_after_write: true,
    };

    let results = run_wal_dst_batch(0..1000, config);
    let summary = summarize_wal_dst_batch(&results);

    let failed: Vec<_> = results.iter().filter(|r| !r.passed).collect();
    assert!(
        failed.is_empty(),
        "Failed {} out of 1000 seeds.\n{}",
        failed.len(),
        summary
    );

    println!("{}", summary);
}
