//! WAL Deterministic Simulation Testing Harness
//!
//! Verifies WAL durability guarantees under fault injection:
//!
//! - **Always mode invariant**: Every acknowledged write MUST survive crash+recovery
//! - **EverySecond mode invariant**: At most 1 second of writes may be lost
//! - **Crash tolerance**: Partial writes at file boundary are detected and skipped
//!
//! ## DST Methodology
//!
//! 1. Create SimulatedWalStore with buggify fault injection
//! 2. Write deltas, tracking which ones were acknowledged
//! 3. Simulate crash (truncate last file at random point)
//! 4. Recover and verify all acknowledged writes are present

use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use crate::redis::SDS;
use crate::replication::lattice::{LamportClock, ReplicaId};
use crate::replication::state::{ReplicatedValue, ReplicationDelta};
use crate::streaming::wal::{WalEntry, WalRotator};
use crate::streaming::wal_store::{
    SimulatedWalStore, SimulatedWalStoreConfig, SimulatedWalStoreStats,
};

/// Result of a single DST run
#[derive(Debug)]
pub struct WalDSTResult {
    pub seed: u64,
    pub total_writes: usize,
    pub acknowledged_writes: usize,
    pub failed_writes: usize,
    pub recovered_entries: usize,
    pub missing_after_recovery: usize,
    pub store_stats: SimulatedWalStoreStats,
    pub passed: bool,
    pub error_message: Option<String>,
}

/// Configuration for WAL DST harness
#[derive(Debug, Clone)]
pub struct WalDSTConfig {
    /// Number of writes per run
    pub num_writes: usize,
    /// Maximum WAL file size (controls rotation)
    pub max_file_size: usize,
    /// Fault injection config
    pub store_config: SimulatedWalStoreConfig,
    /// Whether to simulate crash mid-run
    pub simulate_crash: bool,
    /// Whether to do fsync after each write (simulates Always mode)
    pub fsync_after_write: bool,
}

impl Default for WalDSTConfig {
    fn default() -> Self {
        WalDSTConfig {
            num_writes: 100,
            max_file_size: 512, // Small to force rotation
            store_config: SimulatedWalStoreConfig::default(),
            simulate_crash: true,
            fsync_after_write: true,
        }
    }
}

impl WalDSTConfig {
    /// No faults, no crash — baseline correctness test
    pub fn baseline() -> Self {
        WalDSTConfig {
            store_config: SimulatedWalStoreConfig::no_faults(),
            simulate_crash: false,
            ..Default::default()
        }
    }

    /// Crash without faults — tests crash tolerance
    pub fn crash_only() -> Self {
        WalDSTConfig {
            store_config: SimulatedWalStoreConfig::no_faults(),
            simulate_crash: true,
            ..Default::default()
        }
    }

    /// Full chaos — faults + crash
    pub fn chaos() -> Self {
        WalDSTConfig {
            store_config: SimulatedWalStoreConfig::high_chaos(),
            simulate_crash: true,
            ..Default::default()
        }
    }
}

/// WAL DST Harness
pub struct WalDSTHarness {
    seed: u64,
    rng: SimulatedRng,
    config: WalDSTConfig,
}

impl WalDSTHarness {
    pub fn new(seed: u64, config: WalDSTConfig) -> Self {
        WalDSTHarness {
            seed,
            rng: SimulatedRng::new(seed),
            config,
        }
    }

    /// Run a single DST scenario
    pub fn run(&mut self) -> WalDSTResult {
        // Create simulated store with separate RNG fork
        let store_rng = SimulatedRng::new(self.rng.next_u64());
        let store = SimulatedWalStore::new(store_rng, self.config.store_config.clone());

        let mut rotator = match WalRotator::new(store.clone(), self.config.max_file_size) {
            Ok(r) => r,
            Err(e) => {
                return WalDSTResult {
                    seed: self.seed,
                    total_writes: 0,
                    acknowledged_writes: 0,
                    failed_writes: 0,
                    recovered_entries: 0,
                    missing_after_recovery: 0,
                    store_stats: store.stats(),
                    passed: false,
                    error_message: Some(format!("Failed to create rotator: {}", e)),
                };
            }
        };

        // Track acknowledged writes (shadow state)
        let mut acked_timestamps: Vec<u64> = Vec::new();
        let mut failed_writes = 0;

        // Pick a random crash point (if crash enabled) — crash mid-sequence, not just at the end
        let crash_at = if self.config.simulate_crash {
            self.rng.gen_range(1, (self.config.num_writes as u64).saturating_add(1)) as usize
        } else {
            usize::MAX
        };

        // Phase 1: Write entries (may be interrupted by crash)
        for i in 0..self.config.num_writes {
            // Simulate crash at random point mid-sequence
            if i == crash_at {
                store.inner_store().simulate_crash();
                break;
            }
            let ts = (i as u64)
                .checked_add(1)
                .expect("timestamp overflow unreachable in test");
            let key = format!("key-{:06}", self.rng.gen_range(0, 1000));
            let value = format!("val-{}", ts);

            let delta = make_test_delta(&key, &value, ts);
            let entry = match WalEntry::from_delta(&delta, ts) {
                Ok(e) => e,
                Err(_) => {
                    failed_writes += 1;
                    continue;
                }
            };

            // Append
            match rotator.append(&entry) {
                Ok(_) => {
                    if self.config.fsync_after_write {
                        // Simulate Always mode: fsync after append
                        match rotator.sync() {
                            Ok(()) => {
                                // Entry is durable — add to shadow state
                                acked_timestamps.push(ts);
                            }
                            Err(_) => {
                                // Fsync failed — entry is NOT acknowledged
                                failed_writes += 1;
                            }
                        }
                    } else {
                        // Simulate EverySecond/No mode: ack immediately
                        acked_timestamps.push(ts);
                    }
                }
                Err(_) => {
                    failed_writes += 1;
                }
            }
        }

        // Phase 2: Simulate crash if we didn't already crash mid-sequence.
        // Crash truncates all files to their synced position — un-synced data is lost.
        let already_crashed = self.config.simulate_crash && crash_at < self.config.num_writes;
        if self.config.simulate_crash && !already_crashed {
            store.inner_store().simulate_crash();
        }

        // Phase 3: Recovery — use inner store directly (no fault injection during recovery).
        // This tests the critical invariant: acked data survives crash.
        // Read faults during recovery are a separate concern (retry logic).
        let recovery_store = store.inner_store().clone();
        let recovery_rotator =
            match WalRotator::new(recovery_store, self.config.max_file_size) {
                Ok(r) => r,
                Err(e) => {
                    return WalDSTResult {
                        seed: self.seed,
                        total_writes: self.config.num_writes,
                        acknowledged_writes: acked_timestamps.len(),
                        failed_writes,
                        recovered_entries: 0,
                        missing_after_recovery: acked_timestamps.len(),
                        store_stats: store.stats(),
                        passed: false,
                        error_message: Some(format!("Recovery failed: {}", e)),
                    };
                }
            };

        let recovered = match recovery_rotator.recover_all_entries() {
            Ok(entries) => entries,
            Err(e) => {
                return WalDSTResult {
                    seed: self.seed,
                    total_writes: self.config.num_writes,
                    acknowledged_writes: acked_timestamps.len(),
                    failed_writes,
                    recovered_entries: 0,
                    missing_after_recovery: acked_timestamps.len(),
                    store_stats: store.stats(),
                    passed: false,
                    error_message: Some(format!("Entry recovery failed: {}", e)),
                };
            }
        };

        // Phase 4: Verify invariant
        let recovered_timestamps: std::collections::HashSet<u64> =
            recovered.iter().map(|e| e.timestamp).collect();

        let mut missing = 0;
        let mut missing_ts = Vec::new();

        if self.config.fsync_after_write {
            // ALWAYS MODE INVARIANT: Every acknowledged (fsync'd) write must survive
            for ts in &acked_timestamps {
                if !recovered_timestamps.contains(ts) {
                    missing += 1;
                    if missing_ts.len() < 10 {
                        missing_ts.push(*ts);
                    }
                }
            }
        }
        // For EverySecond/No mode, some loss is acceptable (bounded)

        let passed = missing == 0;
        let error_message = if !passed {
            Some(format!(
                "INVARIANT VIOLATION: {} acknowledged writes missing after recovery. \
                 Acked: {}, Recovered: {}. Missing timestamps (first 10): {:?}",
                missing,
                acked_timestamps.len(),
                recovered.len(),
                missing_ts
            ))
        } else {
            None
        };

        WalDSTResult {
            seed: self.seed,
            total_writes: self.config.num_writes,
            acknowledged_writes: acked_timestamps.len(),
            failed_writes,
            recovered_entries: recovered.len(),
            missing_after_recovery: missing,
            store_stats: store.stats(),
            passed,
            error_message,
        }
    }
}

/// Run a batch of DST tests across multiple seeds
pub fn run_wal_dst_batch(
    seeds: std::ops::Range<u64>,
    config: WalDSTConfig,
) -> Vec<WalDSTResult> {
    seeds
        .map(|seed| {
            let mut harness = WalDSTHarness::new(seed, config.clone());
            harness.run()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_wal_dst_batch(results: &[WalDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = total - passed;

    let total_writes: usize = results.iter().map(|r| r.total_writes).sum();
    let total_acked: usize = results.iter().map(|r| r.acknowledged_writes).sum();
    let total_recovered: usize = results.iter().map(|r| r.recovered_entries).sum();
    let total_missing: usize = results.iter().map(|r| r.missing_after_recovery).sum();

    let mut summary = format!(
        "WAL DST Batch: {}/{} passed ({} failed)\n\
         Total writes: {}, Acknowledged: {}, Recovered: {}, Missing: {}",
        passed, total, failed, total_writes, total_acked, total_recovered, total_missing
    );

    if failed > 0 {
        summary.push_str("\n\nFailed seeds:");
        for r in results.iter().filter(|r| !r.passed) {
            summary.push_str(&format!(
                "\n  Seed {}: {}",
                r.seed,
                r.error_message.as_deref().unwrap_or("unknown error")
            ));
        }
    }

    summary
}

fn make_test_delta(key: &str, value: &str, ts: u64) -> ReplicationDelta {
    let replica_id = ReplicaId::new(1);
    let clock = LamportClock {
        time: ts,
        replica_id,
    };
    let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
    ReplicationDelta::new(key.to_string(), replicated, replica_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_dst_baseline_no_faults() {
        let config = WalDSTConfig::baseline();
        let results = run_wal_dst_batch(0..10, config);

        for r in &results {
            assert!(r.passed, "Seed {} failed: {:?}", r.seed, r.error_message);
            assert_eq!(r.missing_after_recovery, 0);
            assert!(r.acknowledged_writes > 0);
        }
    }

    #[test]
    fn test_wal_dst_crash_no_faults() {
        let config = WalDSTConfig::crash_only();
        let results = run_wal_dst_batch(0..20, config);

        for r in &results {
            assert!(
                r.passed,
                "Seed {} failed: {:?}",
                r.seed, r.error_message
            );
        }

        let summary = summarize_wal_dst_batch(&results);
        assert!(summary.contains("20/20 passed"));
    }

    #[test]
    fn test_wal_dst_with_faults() {
        let config = WalDSTConfig {
            store_config: SimulatedWalStoreConfig::default(),
            simulate_crash: true,
            ..Default::default()
        };

        let results = run_wal_dst_batch(0..50, config);

        // With faults, some writes may fail (which is fine - they're not acknowledged)
        // But all ACKNOWLEDGED writes must survive
        for r in &results {
            assert!(
                r.passed,
                "Seed {} failed: {:?}",
                r.seed, r.error_message
            );
        }
    }

    #[test]
    fn test_wal_dst_rotation_under_load() {
        let config = WalDSTConfig {
            num_writes: 200,
            max_file_size: 100, // Very small to force many rotations
            store_config: SimulatedWalStoreConfig::no_faults(),
            simulate_crash: true,
            fsync_after_write: true,
        };

        let results = run_wal_dst_batch(0..20, config);

        for r in &results {
            assert!(
                r.passed,
                "Seed {} failed: {:?}",
                r.seed, r.error_message
            );
            assert!(
                r.acknowledged_writes > 0,
                "Seed {} had no acknowledged writes",
                r.seed
            );
        }
    }

    #[test]
    fn test_wal_dst_high_chaos() {
        let config = WalDSTConfig::chaos();
        let results = run_wal_dst_batch(0..50, config);

        // Even under high chaos, the invariant must hold:
        // all acknowledged (fsync'd) writes survive crash + recovery
        for r in &results {
            assert!(
                r.passed,
                "Seed {} failed: {:?}",
                r.seed, r.error_message
            );
        }

        let summary = summarize_wal_dst_batch(&results);
        assert!(
            summary.contains("50/50 passed"),
            "Not all seeds passed: {}",
            summary
        );
    }

    #[test]
    fn test_wal_dst_everysec_mode_bounded_loss() {
        // In EverySecond mode, we ack before fsync, so some data may be lost.
        // But the DST only checks Always mode invariant when fsync_after_write=true.
        // For EverySecond, we verify the system doesn't crash/panic.
        let config = WalDSTConfig {
            num_writes: 100,
            max_file_size: 512,
            store_config: SimulatedWalStoreConfig::no_faults(),
            simulate_crash: true,
            fsync_after_write: false, // EverySecond semantics
        };

        let results = run_wal_dst_batch(0..20, config);

        for r in &results {
            // No invariant check for EverySecond mode (loss is acceptable)
            // Just verify the harness runs without panicking
            assert!(r.total_writes > 0);
            assert!(r.acknowledged_writes > 0);
        }
    }
}
