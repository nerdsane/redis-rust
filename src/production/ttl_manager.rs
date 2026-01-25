//! TtlManagerActor - Background actor for TTL key expiration
//!
//! This actor periodically sends eviction messages to shard actors.
//! Follows the actor pattern with proper shutdown handling.
//!
//! ## Design (TigerBeetle/FoundationDB inspired)
//!
//! ```text
//! ┌─────────────────────┐     ┌─────────────────────┐
//! │  TtlManagerActor    │────▶│   ShardActors[N]    │
//! │ (periodic eviction) │     │ (execute eviction)  │
//! └─────────────────────┘     └─────────────────────┘
//! ```

use super::ShardedActorState;
use crate::io::TimeSource;
use crate::observability::Metrics;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, Duration};
use tracing::debug;

const TTL_CHECK_INTERVAL_MS: u64 = 100;
const SHUTDOWN_CHECK_INTERVAL_MS: u64 = 50;

/// Messages for controlling the TtlManagerActor
#[derive(Debug)]
pub enum TtlMessage {
    /// Trigger an immediate eviction check
    Tick,
    /// Graceful shutdown
    Shutdown { response: oneshot::Sender<()> },
}

/// Handle for communicating with the TtlManagerActor
#[derive(Clone)]
pub struct TtlManagerHandle {
    tx: mpsc::UnboundedSender<TtlMessage>,
}

impl TtlManagerHandle {
    /// Trigger an immediate eviction check
    #[inline]
    pub fn tick(&self) {
        let _ = self.tx.send(TtlMessage::Tick);
    }

    /// Graceful shutdown - waits for confirmation
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(TtlMessage::Shutdown { response: tx }).is_ok() {
            let _ = rx.await;
        }
    }

    /// Check if the actor is still running
    pub fn is_running(&self) -> bool {
        !self.tx.is_closed()
    }
}

/// TtlManagerActor with message-based control and proper shutdown
pub struct TtlManagerActor<T: TimeSource> {
    state: ShardedActorState<T>,
    interval_ms: u64,
    metrics: Arc<Metrics>,
    rx: mpsc::UnboundedReceiver<TtlMessage>,
}

impl<T: TimeSource + Clone + Send + 'static> TtlManagerActor<T> {
    /// Create a new TtlManagerActor with default interval
    #[inline]
    pub fn new(state: ShardedActorState<T>, metrics: Arc<Metrics>) -> (TtlManagerHandle, Self) {
        Self::with_interval(state, TTL_CHECK_INTERVAL_MS, metrics)
    }

    /// Create a new TtlManagerActor with custom interval
    #[inline]
    pub fn with_interval(
        state: ShardedActorState<T>,
        interval_ms: u64,
        metrics: Arc<Metrics>,
    ) -> (TtlManagerHandle, Self) {
        debug_assert!(
            interval_ms > 0,
            "Precondition: TTL interval must be positive"
        );

        let (tx, rx) = mpsc::unbounded_channel();
        let handle = TtlManagerHandle { tx };
        let actor = TtlManagerActor {
            state,
            interval_ms,
            metrics,
            rx,
        };

        (handle, actor)
    }

    /// Spawn the actor and return the handle
    pub fn spawn(state: ShardedActorState<T>, metrics: Arc<Metrics>) -> TtlManagerHandle {
        let (handle, actor) = Self::new(state, metrics);
        tokio::spawn(actor.run());
        handle
    }

    /// Spawn with custom interval
    pub fn spawn_with_interval(
        state: ShardedActorState<T>,
        interval_ms: u64,
        metrics: Arc<Metrics>,
    ) -> TtlManagerHandle {
        let (handle, actor) = Self::with_interval(state, interval_ms, metrics);
        tokio::spawn(actor.run());
        handle
    }

    /// Run the actor's main loop
    ///
    /// This loop:
    /// 1. Checks for messages (shutdown, manual tick)
    /// 2. Periodically triggers eviction on all shards
    /// 3. Responds to shutdown within SHUTDOWN_CHECK_INTERVAL_MS
    pub async fn run(mut self) {
        let mut tick_interval = interval(Duration::from_millis(self.interval_ms));
        let shutdown_check = Duration::from_millis(SHUTDOWN_CHECK_INTERVAL_MS);

        loop {
            tokio::select! {
                // Check for control messages
                msg = self.rx.recv() => {
                    match msg {
                        Some(TtlMessage::Tick) => {
                            self.do_eviction().await;
                        }
                        Some(TtlMessage::Shutdown { response }) => {
                            debug!("TTL manager shutting down");
                            let _ = response.send(());
                            break;
                        }
                        None => {
                            // Channel closed, shutdown
                            debug!("TTL manager channel closed, shutting down");
                            break;
                        }
                    }
                }

                // Periodic eviction
                _ = tick_interval.tick() => {
                    self.do_eviction().await;
                }

                // Ensure we're responsive to shutdown even during long intervals
                _ = tokio::time::sleep(shutdown_check) => {
                    // Just a wakeup to check messages
                }
            }
        }
    }

    /// Perform eviction on all shards
    async fn do_eviction(&self) {
        let evicted = self.state.evict_expired_all_shards().await;
        if evicted > 0 {
            debug!("TTL manager evicted {} expired keys", evicted);
            self.metrics.record_ttl_eviction(evicted);
        }

        // Postcondition: evicted count should be reasonable
        debug_assert!(
            evicted < 1_000_000,
            "Postcondition: evicted count {} seems unreasonably high",
            evicted
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::ProductionTimeSource;
    use crate::observability::DatadogConfig;

    fn test_metrics() -> Arc<Metrics> {
        Arc::new(Metrics::new(&DatadogConfig))
    }

    #[tokio::test]
    async fn test_ttl_manager_shutdown() {
        let state = ShardedActorState::<ProductionTimeSource>::with_shards(2);
        let handle = TtlManagerActor::spawn(state, test_metrics());

        // Should be running
        assert!(handle.is_running());

        // Shutdown
        handle.shutdown().await;

        // Give it a moment to fully stop
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Channel should be closed now
        assert!(!handle.is_running());
    }

    #[tokio::test]
    async fn test_ttl_manager_manual_tick() {
        let state = ShardedActorState::<ProductionTimeSource>::with_shards(2);
        let handle = TtlManagerActor::spawn(state, test_metrics());

        // Manual tick should not panic
        handle.tick();
        handle.tick();
        handle.tick();

        // Cleanup
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_ttl_manager_custom_interval() {
        let state = ShardedActorState::<ProductionTimeSource>::with_shards(1);
        let handle = TtlManagerActor::spawn_with_interval(state, 50, test_metrics());

        // Let it run a few cycles
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should still be running
        assert!(handle.is_running());

        // Cleanup
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_ttl_manager_multiple_handles() {
        let state = ShardedActorState::<ProductionTimeSource>::with_shards(2);
        let handle1 = TtlManagerActor::spawn(state, test_metrics());
        let handle2 = handle1.clone();

        // Both handles should work
        handle1.tick();
        handle2.tick();

        // Both should see running state
        assert!(handle1.is_running());
        assert!(handle2.is_running());

        // Shutdown via either handle
        handle1.shutdown().await;

        // Both should now see closed
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!handle1.is_running());
        assert!(!handle2.is_running());
    }
}
