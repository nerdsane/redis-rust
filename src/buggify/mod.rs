//! BUGGIFY - FoundationDB-Style Fault Injection System
//!
//! This module provides deterministic, reproducible fault injection inspired by
//! FoundationDB's BUGGIFY system. The key insight is that every decision point
//! in the code can be a potential fault injection site.
//!
//! # Usage
//!
//! ```ignore
//! use crate::buggify;
//!
//! // Basic usage - inject fault with default probability
//! if buggify!(rng, faults::network::PACKET_DROP) {
//!     return; // Drop the packet
//! }
//!
//! // With custom probability
//! if buggify!(rng, faults::network::DELAY, 0.10) {
//!     sleep(random_delay()).await;
//! }
//!
//! // Location-based (auto-generates fault ID from file:line)
//! if buggify_here!(rng) {
//!     corrupt_data(&mut buffer);
//! }
//! ```
//!
//! # Design Principles
//!
//! 1. **Deterministic**: Given the same seed, faults occur in the same places
//! 2. **Reproducible**: Failed test can replay exact fault sequence
//! 3. **Configurable**: Per-fault probabilities, global multipliers
//! 4. **Zero production overhead**: Compiles to nothing in production builds

pub mod faults;
pub mod config;

pub use config::FaultConfig;
pub use faults::ALL_FAULTS;

use std::cell::RefCell;
use std::collections::HashMap;

/// Statistics tracking for fault injection
#[derive(Debug, Clone, Default)]
pub struct BuggifyStats {
    /// Number of times each fault was checked
    pub checks: HashMap<String, u64>,
    /// Number of times each fault was triggered
    pub triggers: HashMap<String, u64>,
}

impl BuggifyStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_check(&mut self, fault_id: &str) {
        *self.checks.entry(fault_id.to_string()).or_insert(0) += 1;
    }

    pub fn record_trigger(&mut self, fault_id: &str) {
        *self.triggers.entry(fault_id.to_string()).or_insert(0) += 1;
    }

    pub fn trigger_rate(&self, fault_id: &str) -> f64 {
        let checks = self.checks.get(fault_id).copied().unwrap_or(0);
        let triggers = self.triggers.get(fault_id).copied().unwrap_or(0);
        if checks == 0 {
            0.0
        } else {
            triggers as f64 / checks as f64
        }
    }

    pub fn merge(&mut self, other: &BuggifyStats) {
        for (k, v) in &other.checks {
            *self.checks.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &other.triggers {
            *self.triggers.entry(k.clone()).or_insert(0) += v;
        }
    }

    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        lines.push("BUGGIFY Statistics:".to_string());

        let mut sorted_faults: Vec<_> = self.checks.keys().collect();
        sorted_faults.sort();

        for fault_id in sorted_faults {
            let checks = self.checks.get(fault_id).copied().unwrap_or(0);
            let triggers = self.triggers.get(fault_id).copied().unwrap_or(0);
            let rate = self.trigger_rate(fault_id);
            lines.push(format!(
                "  {}: {}/{} ({:.2}%)",
                fault_id,
                triggers,
                checks,
                rate * 100.0
            ));
        }

        lines.join("\n")
    }
}

// Thread-local buggify context for tracking and configuration
thread_local! {
    static BUGGIFY_CONTEXT: RefCell<BuggifyContext> = RefCell::new(BuggifyContext::default());
}

/// Per-thread buggify context
#[derive(Debug, Default)]
pub struct BuggifyContext {
    pub config: FaultConfig,
    pub stats: BuggifyStats,
    /// When true, all buggify calls return false (for critical sections)
    pub suppressed: bool,
}

impl BuggifyContext {
    pub fn new(config: FaultConfig) -> Self {
        BuggifyContext {
            config,
            stats: BuggifyStats::new(),
            suppressed: false,
        }
    }
}

/// Set the buggify configuration for the current thread
pub fn set_config(config: FaultConfig) {
    BUGGIFY_CONTEXT.with(|ctx| {
        ctx.borrow_mut().config = config;
    });
}

/// Get current buggify stats for the thread
pub fn get_stats() -> BuggifyStats {
    BUGGIFY_CONTEXT.with(|ctx| ctx.borrow().stats.clone())
}

/// Reset stats for the current thread
pub fn reset_stats() {
    BUGGIFY_CONTEXT.with(|ctx| {
        ctx.borrow_mut().stats = BuggifyStats::new();
    });
}

/// Suppress buggify for the current scope (for critical sections)
pub struct BuggifySuppressor;

impl BuggifySuppressor {
    pub fn new() -> Self {
        BUGGIFY_CONTEXT.with(|ctx| {
            ctx.borrow_mut().suppressed = true;
        });
        BuggifySuppressor
    }
}

impl Default for BuggifySuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BuggifySuppressor {
    fn drop(&mut self) {
        BUGGIFY_CONTEXT.with(|ctx| {
            ctx.borrow_mut().suppressed = false;
        });
    }
}

/// Core buggify check function - called by macros
///
/// Returns true if the fault should be injected.
/// Uses the provided RNG for deterministic behavior.
#[inline]
pub fn should_buggify<R: crate::io::Rng>(rng: &mut R, fault_id: &str) -> bool {
    BUGGIFY_CONTEXT.with(|ctx| {
        let mut ctx = ctx.borrow_mut();

        // Record the check
        ctx.stats.record_check(fault_id);

        // Check if suppressed
        if ctx.suppressed {
            return false;
        }

        // Get probability and check
        let prob = ctx.config.get(fault_id);
        if prob <= 0.0 {
            return false;
        }

        // Use deterministic RNG
        let random_value = rng.gen_range(0, 1_000_000) as f64 / 1_000_000.0;
        let triggered = random_value < prob;

        if triggered {
            ctx.stats.record_trigger(fault_id);
        }

        triggered
    })
}

/// Check buggify with custom probability override
#[inline]
pub fn should_buggify_with_prob<R: crate::io::Rng>(
    rng: &mut R,
    fault_id: &str,
    probability: f64,
) -> bool {
    BUGGIFY_CONTEXT.with(|ctx| {
        let mut ctx = ctx.borrow_mut();

        ctx.stats.record_check(fault_id);

        if ctx.suppressed || !ctx.config.enabled {
            return false;
        }

        let random_value = rng.gen_range(0, 1_000_000) as f64 / 1_000_000.0;
        let triggered = random_value < probability.clamp(0.0, 1.0);

        if triggered {
            ctx.stats.record_trigger(fault_id);
        }

        triggered
    })
}

/// BUGGIFY macro - the main interface for fault injection
///
/// # Examples
///
/// ```ignore
/// // Check if fault should be injected (uses configured probability)
/// if buggify!(rng, faults::network::PACKET_DROP) {
///     return; // Drop the packet
/// }
///
/// // With explicit probability override
/// if buggify!(rng, faults::network::DELAY, 0.10) {
///     delay_packet(100);
/// }
/// ```
#[macro_export]
macro_rules! buggify {
    ($rng:expr, $fault_id:expr) => {
        $crate::buggify::should_buggify($rng, $fault_id)
    };
    ($rng:expr, $fault_id:expr, $prob:expr) => {
        $crate::buggify::should_buggify_with_prob($rng, $fault_id, $prob)
    };
}

/// BUGGIFY_HERE macro - auto-generates fault ID from source location
///
/// Useful for ad-hoc injection points where you don't want to define
/// a named fault constant.
#[macro_export]
macro_rules! buggify_here {
    ($rng:expr) => {
        $crate::buggify::should_buggify($rng, concat!(file!(), ":", line!()))
    };
    ($rng:expr, $prob:expr) => {
        $crate::buggify::should_buggify_with_prob($rng, concat!(file!(), ":", line!()), $prob)
    };
}

/// BUGGIFY_RARELY macro - for very rare faults (default 0.1%)
#[macro_export]
macro_rules! buggify_rarely {
    ($rng:expr, $fault_id:expr) => {
        $crate::buggify::should_buggify_with_prob($rng, $fault_id, 0.001)
    };
}

/// BUGGIFY_SOMETIMES macro - for occasional faults (default 5%)
#[macro_export]
macro_rules! buggify_sometimes {
    ($rng:expr, $fault_id:expr) => {
        $crate::buggify::should_buggify_with_prob($rng, $fault_id, 0.05)
    };
}

/// BUGGIFY_OFTEN macro - for frequent faults (default 20%)
#[macro_export]
macro_rules! buggify_often {
    ($rng:expr, $fault_id:expr) => {
        $crate::buggify::should_buggify_with_prob($rng, $fault_id, 0.20)
    };
}

/// Suppress all buggify calls within a scope
///
/// # Example
///
/// ```ignore
/// {
///     let _guard = suppress_buggify!();
///     // Critical section - no faults injected
///     do_critical_work();
/// } // buggify re-enabled when _guard drops
/// ```
#[macro_export]
macro_rules! suppress_buggify {
    () => {
        $crate::buggify::BuggifySuppressor::new()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::production::ProductionRng;

    #[test]
    fn test_buggify_disabled() {
        set_config(FaultConfig::disabled());
        let mut rng = ProductionRng::new();

        // Should never trigger when disabled
        for _ in 0..1000 {
            assert!(!buggify!(&mut rng, faults::network::PACKET_DROP));
        }
    }

    #[test]
    fn test_buggify_with_prob() {
        set_config(FaultConfig::new());
        let mut rng = ProductionRng::new();

        // With 100% probability should always trigger
        let mut triggered = 0;
        for _ in 0..100 {
            if buggify!(&mut rng, "test.always", 1.0) {
                triggered += 1;
            }
        }
        assert_eq!(triggered, 100);

        // With 0% probability should never trigger
        triggered = 0;
        for _ in 0..100 {
            if buggify!(&mut rng, "test.never", 0.0) {
                triggered += 1;
            }
        }
        assert_eq!(triggered, 0);
    }

    #[test]
    fn test_buggify_stats() {
        reset_stats();
        set_config(FaultConfig::moderate());
        let mut rng = ProductionRng::new();

        for _ in 0..1000 {
            let _ = buggify!(&mut rng, faults::network::PACKET_DROP);
        }

        let stats = get_stats();
        assert_eq!(stats.checks.get(faults::network::PACKET_DROP), Some(&1000));
        // Triggers should be around 1% (10 +/- some variance)
        let triggers = stats.triggers.get(faults::network::PACKET_DROP).copied().unwrap_or(0);
        assert!(triggers > 0 && triggers < 100, "triggers: {}", triggers);
    }

    #[test]
    fn test_buggify_suppression() {
        set_config(FaultConfig::moderate());
        let mut rng = ProductionRng::new();

        {
            let _guard = suppress_buggify!();
            // Should never trigger while suppressed
            for _ in 0..100 {
                assert!(!buggify!(&mut rng, faults::network::PACKET_DROP, 1.0));
            }
        }

        // After suppressor drops, should trigger again
        assert!(buggify!(&mut rng, "test.after_suppress", 1.0));
    }

    #[test]
    fn test_buggify_here() {
        set_config(FaultConfig::new());
        let mut rng = ProductionRng::new();

        // Should generate unique fault IDs based on location
        let result1 = buggify_here!(&mut rng, 1.0);
        let result2 = buggify_here!(&mut rng, 1.0);

        // Both should trigger with 100% prob
        assert!(result1);
        assert!(result2);
    }

    #[test]
    fn test_convenience_macros() {
        set_config(FaultConfig::new());
        let mut rng = ProductionRng::new();

        // Test the convenience macros compile and run
        let _ = buggify_rarely!(&mut rng, "test.rare");
        let _ = buggify_sometimes!(&mut rng, "test.sometimes");
        let _ = buggify_often!(&mut rng, "test.often");
    }
}
