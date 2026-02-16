//! Clock Abstraction for Deterministic Simulation Testing
//!
//! Provides time abstractions that work in both production (real time)
//! and simulation (virtual time) contexts.
//!
//! ## Design Principles (TigerStyle)
//!
//! 1. **Explicit time**: All time operations go through this trait
//! 2. **No hidden state**: Clock state is visible and controllable
//! 3. **Deterministic**: Same inputs produce same outputs in simulation

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Timestamp in milliseconds
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct StreamingTimestamp(pub u64);

impl StreamingTimestamp {
    pub const ZERO: StreamingTimestamp = StreamingTimestamp(0);

    pub fn from_millis(ms: u64) -> Self {
        StreamingTimestamp(ms)
    }

    pub fn as_millis(&self) -> u64 {
        self.0
    }

    pub fn saturating_sub(&self, other: StreamingTimestamp) -> Duration {
        Duration::from_millis(self.0.saturating_sub(other.0))
    }
}

impl std::ops::Add<Duration> for StreamingTimestamp {
    type Output = StreamingTimestamp;

    fn add(self, rhs: Duration) -> Self::Output {
        StreamingTimestamp(self.0.saturating_add(rhs.as_millis() as u64))
    }
}

/// Clock trait for time operations
///
/// Implementations:
/// - `ProductionClock`: Uses real system time
/// - `SimulatedClock`: Uses controlled virtual time for DST
pub trait StreamingClock: Send + Sync + Clone + 'static {
    /// Get current time
    fn now(&self) -> StreamingTimestamp;

    /// Get elapsed time since a previous timestamp
    fn elapsed(&self, since: StreamingTimestamp) -> Duration {
        let now = self.now();
        Duration::from_millis(now.0.saturating_sub(since.0))
    }

    /// Check if a duration has elapsed since a timestamp
    fn has_elapsed(&self, since: StreamingTimestamp, duration: Duration) -> bool {
        self.elapsed(since) >= duration
    }
}

/// Production clock using real system time
#[derive(Clone)]
pub struct ProductionClock {
    /// Epoch instant for calculating timestamps
    start: Instant,
    /// Initial timestamp in milliseconds
    start_millis: u64,
}

impl Default for ProductionClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ProductionClock {
    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let start_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX_EPOCH")
            .as_millis() as u64;
        ProductionClock {
            start: Instant::now(),
            start_millis,
        }
    }
}

impl StreamingClock for ProductionClock {
    fn now(&self) -> StreamingTimestamp {
        let elapsed = self.start.elapsed().as_millis() as u64;
        StreamingTimestamp(self.start_millis + elapsed)
    }
}

/// Simulated clock for deterministic testing
///
/// Time only advances when explicitly told to via `advance()` or `set()`.
#[derive(Clone)]
pub struct SimulatedClock {
    /// Current virtual time in milliseconds
    time_ms: Arc<AtomicU64>,
}

impl Default for SimulatedClock {
    fn default() -> Self {
        Self::new(0)
    }
}

impl SimulatedClock {
    /// Create a new simulated clock starting at the given time
    pub fn new(start_ms: u64) -> Self {
        SimulatedClock {
            time_ms: Arc::new(AtomicU64::new(start_ms)),
        }
    }

    /// Advance time by the given duration
    pub fn advance(&self, duration: Duration) {
        self.time_ms
            .fetch_add(duration.as_millis() as u64, Ordering::SeqCst);
    }

    /// Advance time by milliseconds
    pub fn advance_ms(&self, ms: u64) {
        self.time_ms.fetch_add(ms, Ordering::SeqCst);
    }

    /// Set time to a specific value
    pub fn set(&self, time_ms: u64) {
        self.time_ms.store(time_ms, Ordering::SeqCst);
    }

    /// Get current time in milliseconds
    pub fn current_ms(&self) -> u64 {
        self.time_ms.load(Ordering::SeqCst)
    }
}

impl StreamingClock for SimulatedClock {
    fn now(&self) -> StreamingTimestamp {
        StreamingTimestamp(self.time_ms.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_production_clock() {
        let clock = ProductionClock::new();
        let t1 = clock.now();
        std::thread::sleep(Duration::from_millis(10));
        let t2 = clock.now();

        assert!(t2.0 > t1.0, "Time should advance");
        assert!(t2.0 - t1.0 >= 10, "Should have elapsed at least 10ms");
    }

    #[test]
    fn test_simulated_clock_deterministic() {
        let clock = SimulatedClock::new(1000);

        // Time doesn't advance on its own
        let t1 = clock.now();
        let t2 = clock.now();
        assert_eq!(t1, t2, "Time should not advance without explicit call");

        // Explicit advance
        clock.advance_ms(100);
        let t3 = clock.now();
        assert_eq!(t3.0, 1100);

        // Set to specific time
        clock.set(5000);
        assert_eq!(clock.now().0, 5000);
    }

    #[test]
    fn test_simulated_clock_shared() {
        let clock = SimulatedClock::new(0);
        let clock2 = clock.clone();

        clock.advance_ms(100);
        assert_eq!(clock2.now().0, 100, "Clones should share state");
    }

    #[test]
    fn test_elapsed() {
        let clock = SimulatedClock::new(1000);
        let start = clock.now();

        clock.advance_ms(250);

        let elapsed = clock.elapsed(start);
        assert_eq!(elapsed, Duration::from_millis(250));
        assert!(clock.has_elapsed(start, Duration::from_millis(200)));
        assert!(!clock.has_elapsed(start, Duration::from_millis(300)));
    }

    #[test]
    fn test_timestamp_arithmetic() {
        let ts = StreamingTimestamp::from_millis(1000);
        let ts2 = ts + Duration::from_millis(500);
        assert_eq!(ts2.0, 1500);

        let diff = ts2.saturating_sub(ts);
        assert_eq!(diff, Duration::from_millis(500));
    }
}
