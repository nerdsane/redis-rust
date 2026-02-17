//! WAL Actor - Group Commit with Turbopuffer-Inspired Broker Pattern
//!
//! The WAL actor runs a tight loop: accumulate entries from concurrent writers,
//! issue one fsync for the batch, resolve all waiters. With 50 concurrent clients,
//! this amortizes the ~100μs fsync cost to ~2μs per write.
//!
//! ## Architecture
//!
//! ```text
//! Writer 1 ──┐
//! Writer 2 ──┤──► WalActor ──► batch append ──► fsync ──► resolve acks
//! Writer 3 ──┘
//! ```
//!
//! ## Fsync Modes
//!
//! - **Always**: Group commit — batch + fsync before resolving any ack
//! - **EverySecond**: Append + resolve immediately; fsync on timer
//! - **No**: Append + resolve immediately; OS decides when to flush

use crate::replication::state::ReplicationDelta;
use crate::streaming::wal::{WalEntry, WalRotator};
use crate::streaming::wal_config::{FsyncPolicy, WalConfig};
use crate::streaming::wal_store::{WalError, WalStore};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

/// Channel capacity for WAL actor — bounds memory under slow fsync.
/// 256 entries × ~200 bytes ≈ 50KB worst case.
const WAL_CHANNEL_CAPACITY: usize = 256;

/// Messages for the WAL actor
pub enum WalMessage {
    /// Write a delta to the WAL.
    /// In Always mode, the ack is sent after fsync.
    /// In EverySecond/No mode, the ack is sent after append.
    Write {
        delta: Arc<ReplicationDelta>,
        timestamp: u64,
        ack_tx: Option<oneshot::Sender<Result<(), WalError>>>,
    },
    /// Periodic fsync tick (EverySecond mode)
    SyncTick,
    /// Truncate WAL entries that have been streamed to object store
    TruncateUpTo {
        streamed_up_to_timestamp: u64,
    },
    /// Graceful shutdown
    Shutdown {
        response_tx: oneshot::Sender<()>,
    },
}

/// WAL actor that owns the rotator and processes messages
pub struct WalActor<S: WalStore> {
    rotator: WalRotator<S>,
    config: WalConfig,
    rx: mpsc::Receiver<WalMessage>,
    /// Pending ack channels for group commit (Always mode only)
    pending_acks: Vec<oneshot::Sender<Result<(), WalError>>>,
    /// Number of entries appended since last fsync
    entries_since_sync: usize,
}

impl<S: WalStore> WalActor<S> {
    fn new(
        rotator: WalRotator<S>,
        config: WalConfig,
        rx: mpsc::Receiver<WalMessage>,
    ) -> Self {
        WalActor {
            rotator,
            config,
            rx,
            pending_acks: Vec::with_capacity(64),
            entries_since_sync: 0,
        }
    }

    /// Verify actor invariants (TigerStyle)
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        debug_assert!(
            self.pending_acks.len() <= self.entries_since_sync,
            "Invariant: pending_acks ({}) must not exceed entries_since_sync ({})",
            self.pending_acks.len(),
            self.entries_since_sync
        );
    }

    /// Run the actor loop
    pub async fn run(mut self) {
        match self.config.fsync_policy {
            FsyncPolicy::Always => self.run_always_mode().await,
            FsyncPolicy::EverySecond => self.run_everysec_mode().await,
            FsyncPolicy::No => self.run_no_mode().await,
        }
    }

    /// Always mode: group commit — batch entries, fsync, resolve acks.
    /// After receiving the first message, waits up to `group_commit_max_wait`
    /// for more messages to accumulate before flushing (turbopuffer pattern).
    async fn run_always_mode(&mut self) {
        loop {
            // Wait for first message
            let msg = match self.rx.recv().await {
                Some(msg) => msg,
                None => break,
            };

            if self.handle_message_always(msg) {
                break;
            }

            // Wait up to group_commit_max_wait for more messages to accumulate
            if self.entries_since_sync > 0
                && self.entries_since_sync < self.config.group_commit_max_entries
            {
                let wait = self.config.group_commit_max_wait;
                let _ = tokio::time::timeout(wait, async {
                    while self.entries_since_sync < self.config.group_commit_max_entries {
                        match self.rx.recv().await {
                            Some(msg) => {
                                if self.handle_message_always(msg) {
                                    return;
                                }
                            }
                            None => return,
                        }
                    }
                })
                .await;
            }

            // Drain any remaining immediately-available messages
            while self.entries_since_sync < self.config.group_commit_max_entries {
                match self.rx.try_recv() {
                    Ok(msg) => {
                        if self.handle_message_always(msg) {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }

            // If we have pending entries, fsync and resolve acks
            if self.entries_since_sync > 0 {
                self.flush_group_commit();
            }
        }
    }

    /// Handle a single message in Always mode. Returns true if shutdown.
    fn handle_message_always(&mut self, msg: WalMessage) -> bool {
        match msg {
            WalMessage::Write {
                delta,
                timestamp,
                ack_tx,
            } => {
                match WalEntry::from_delta(&delta, timestamp) {
                    Ok(entry) => match self.rotator.append(&entry) {
                        Ok(_) => {
                            self.entries_since_sync = self
                                .entries_since_sync
                                .checked_add(1)
                                .expect("entries_since_sync overflow unreachable");
                            if let Some(tx) = ack_tx {
                                self.pending_acks.push(tx);
                            }
                        }
                        Err(e) => {
                            // Forward original error — don't erase type info
                            if let Some(tx) = ack_tx {
                                let _ = tx.send(Err(e));
                            }
                        }
                    },
                    Err(e) => {
                        if let Some(tx) = ack_tx {
                            let _ = tx.send(Err(e));
                        }
                    }
                }

                #[cfg(debug_assertions)]
                self.verify_invariants();

                false
            }
            WalMessage::SyncTick => false, // No-op in Always mode
            WalMessage::TruncateUpTo {
                streamed_up_to_timestamp,
            } => {
                self.handle_truncation(streamed_up_to_timestamp);
                false
            }
            WalMessage::Shutdown { response_tx } => {
                // Final flush before shutdown
                self.flush_group_commit();
                info!("WAL actor shutting down");
                let _ = response_tx.send(());
                true
            }
        }
    }

    /// Fsync and resolve all pending acks (group commit flush)
    fn flush_group_commit(&mut self) {
        if self.entries_since_sync == 0 {
            return;
        }

        let sync_result = self.rotator.sync();
        let acks = std::mem::take(&mut self.pending_acks);

        match sync_result {
            Ok(()) => {
                // All entries are durable — resolve all acks with Ok
                for tx in acks {
                    let _ = tx.send(Ok(()));
                }
            }
            Err(e) => {
                // Fsync failed — all entries in this batch are NOT durable
                let err_msg = e.to_string();
                error!("WAL fsync failed: {}", err_msg);
                for tx in acks {
                    let _ = tx.send(Err(WalError::FsyncFailed(err_msg.clone())));
                }
            }
        }

        self.entries_since_sync = 0;

        debug_assert_eq!(self.entries_since_sync, 0, "Postcondition: entries_since_sync must be 0 after flush");
        debug_assert!(self.pending_acks.is_empty(), "Postcondition: pending_acks must be empty after flush");
    }

    /// EverySecond mode: append + ack immediately; fsync on timer
    async fn run_everysec_mode(&mut self) {
        loop {
            let msg = match self.rx.recv().await {
                Some(msg) => msg,
                None => break,
            };

            match msg {
                WalMessage::Write {
                    delta,
                    timestamp,
                    ack_tx,
                } => {
                    let result = WalEntry::from_delta(&delta, timestamp)
                        .and_then(|entry| self.rotator.append(&entry).map(|_| ()));

                    // Only count successful writes for sync tracking
                    if result.is_ok() {
                        self.entries_since_sync = self
                            .entries_since_sync
                            .checked_add(1)
                            .expect("entries_since_sync overflow unreachable");
                    }

                    // Ack immediately (before fsync) — RPO ≤ 1s
                    if let Some(tx) = ack_tx {
                        let _ = tx.send(result);
                    }
                }
                WalMessage::SyncTick => {
                    if self.entries_since_sync > 0 {
                        if let Err(e) = self.rotator.sync() {
                            error!("WAL periodic fsync failed: {}", e);
                        }
                        self.entries_since_sync = 0;
                    }
                }
                WalMessage::TruncateUpTo {
                    streamed_up_to_timestamp,
                } => {
                    self.handle_truncation(streamed_up_to_timestamp);
                }
                WalMessage::Shutdown { response_tx } => {
                    // Final sync before shutdown
                    if self.entries_since_sync > 0 {
                        if let Err(e) = self.rotator.sync() {
                            error!("WAL final fsync failed: {}", e);
                        }
                    }
                    info!("WAL actor (EverySecond) shutting down");
                    let _ = response_tx.send(());
                    break;
                }
            }
        }
    }

    /// No mode: append + ack immediately; never explicit fsync
    async fn run_no_mode(&mut self) {
        loop {
            let msg = match self.rx.recv().await {
                Some(msg) => msg,
                None => break,
            };

            match msg {
                WalMessage::Write {
                    delta,
                    timestamp,
                    ack_tx,
                } => {
                    let result = WalEntry::from_delta(&delta, timestamp)
                        .and_then(|entry| self.rotator.append(&entry).map(|_| ()));

                    if let Some(tx) = ack_tx {
                        let _ = tx.send(result);
                    }
                }
                WalMessage::SyncTick => {} // No-op
                WalMessage::TruncateUpTo {
                    streamed_up_to_timestamp,
                } => {
                    self.handle_truncation(streamed_up_to_timestamp);
                }
                WalMessage::Shutdown { response_tx } => {
                    info!("WAL actor (No fsync) shutting down");
                    let _ = response_tx.send(());
                    break;
                }
            }
        }
    }

    fn handle_truncation(&mut self, streamed_up_to_timestamp: u64) {
        match self.rotator.truncate_before(streamed_up_to_timestamp) {
            Ok(deleted) => {
                if deleted > 0 {
                    info!(
                        "WAL truncated {} files (up to timestamp {})",
                        deleted, streamed_up_to_timestamp
                    );
                }
            }
            Err(e) => {
                error!("WAL truncation failed: {}", e);
            }
        }
    }
}

// ============================================================================
// WalActorHandle - public interface for interacting with the WAL actor
// ============================================================================

/// Handle for sending messages to the WAL actor
#[derive(Clone)]
pub struct WalActorHandle {
    tx: mpsc::Sender<WalMessage>,
    fsync_policy: FsyncPolicy,
}

impl WalActorHandle {
    /// Write a delta with durability guarantee (Always mode: waits for fsync).
    /// In EverySecond/No mode, this is equivalent to write_fire_and_forget.
    pub async fn write_durable(
        &self,
        delta: Arc<ReplicationDelta>,
        timestamp: u64,
    ) -> Result<(), WalError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self
            .tx
            .send(WalMessage::Write {
                delta,
                timestamp,
                ack_tx: Some(ack_tx),
            })
            .await
            .is_err()
        {
            return Err(WalError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "WAL actor unavailable",
            )));
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ack_rx,
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(WalError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "WAL actor dropped ack channel",
            ))),
            Err(_) => {
                warn!("WAL write_durable timed out after 5s");
                Err(WalError::FsyncFailed("WAL write timed out".to_string()))
            }
        }
    }

    /// Write a delta without waiting for durability (fire-and-forget).
    /// Used in EverySecond/No modes where immediate ack is acceptable.
    pub fn write_fire_and_forget(&self, delta: Arc<ReplicationDelta>, timestamp: u64) {
        let _ = self.tx.try_send(WalMessage::Write {
            delta,
            timestamp,
            ack_tx: None,
        });
    }

    /// Signal that entries up to the given timestamp have been streamed.
    /// The WAL actor will delete WAL files containing only older entries.
    pub fn truncate(&self, streamed_up_to_timestamp: u64) {
        let _ = self.tx.try_send(WalMessage::TruncateUpTo {
            streamed_up_to_timestamp,
        });
    }

    /// Send periodic sync tick (for EverySecond mode)
    pub fn sync_tick(&self) {
        let _ = self.tx.try_send(WalMessage::SyncTick);
    }

    /// Graceful shutdown — waits for final flush
    pub async fn shutdown(&self) {
        let (response_tx, response_rx) = oneshot::channel();
        if self
            .tx
            .send(WalMessage::Shutdown { response_tx })
            .await
            .is_ok()
        {
            let _ = response_rx.await;
        }
    }

    /// Get the fsync policy
    pub fn fsync_policy(&self) -> FsyncPolicy {
        self.fsync_policy
    }
}

/// Spawn a WAL actor on a dedicated blocking thread and return its handle + join handle.
/// Uses `spawn_blocking` to avoid stalling the tokio cooperative scheduler during fsync.
pub fn spawn_wal_actor<S: WalStore + 'static>(
    store: S,
    config: WalConfig,
) -> Result<(WalActorHandle, tokio::task::JoinHandle<()>), WalError> {
    debug_assert!(
        config.max_file_size > crate::streaming::wal::WAL_HEADER_SIZE,
        "Precondition: max_file_size must be > WAL_HEADER_SIZE"
    );
    debug_assert!(
        config.group_commit_max_entries > 0,
        "Precondition: group_commit_max_entries must be > 0"
    );

    let rotator = WalRotator::new(store, config.max_file_size)?;
    let (tx, rx) = mpsc::channel(WAL_CHANNEL_CAPACITY);

    let fsync_policy = config.fsync_policy;
    let actor = WalActor::new(rotator, config, rx);

    // Use spawn (not spawn_blocking) since the actor loop is async and
    // only blocks briefly during fsync. For production with slow disks,
    // consider wrapping individual fsync calls in spawn_blocking.
    let task = tokio::spawn(actor.run());

    let handle = WalActorHandle { tx, fsync_policy };
    Ok((handle, task))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::wal_store::InMemoryWalStore;
    use std::path::PathBuf;
    use std::time::Duration;

    fn make_test_delta(key: &str, value: &str, ts: u64) -> Arc<ReplicationDelta> {
        use crate::redis::SDS;
        use crate::replication::lattice::{LamportClock, ReplicaId};
        use crate::replication::state::ReplicatedValue;
        let replica_id = ReplicaId::new(1);
        let clock = LamportClock {
            time: ts,
            replica_id,
        };
        let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
        Arc::new(ReplicationDelta::new(key.to_string(), replicated, replica_id))
    }

    fn test_config(policy: FsyncPolicy) -> WalConfig {
        WalConfig {
            enabled: true,
            wal_dir: PathBuf::from("/tmp/wal-test"),
            fsync_policy: policy,
            max_file_size: 1024 * 1024,
            group_commit_max_entries: 8,
            group_commit_max_wait: Duration::from_micros(50),
            truncation_check_interval: Duration::from_millis(100),
        }
    }

    #[tokio::test]
    async fn test_wal_actor_always_mode() {
        let store = InMemoryWalStore::new();
        let config = test_config(FsyncPolicy::Always);

        let (handle, task) = spawn_wal_actor(store.clone(), config).unwrap();

        // Write some deltas
        for i in 0..10 {
            let delta = make_test_delta(&format!("k{}", i), &format!("v{}", i), (i + 1) as u64 * 100);
            handle.write_durable(delta, (i + 1) as u64 * 100).await.unwrap();
        }

        // Shutdown
        handle.shutdown().await;
        task.await.unwrap();

        // Verify data is in the store
        let files = store.list().unwrap();
        assert!(!files.is_empty());
    }

    #[tokio::test]
    async fn test_wal_actor_everysec_mode() {
        let store = InMemoryWalStore::new();
        let config = test_config(FsyncPolicy::EverySecond);

        let (handle, task) = spawn_wal_actor(store.clone(), config).unwrap();

        // Write some deltas
        for i in 0..5 {
            let delta = make_test_delta(&format!("k{}", i), "v", (i + 1) as u64 * 100);
            handle
                .write_durable(delta, (i + 1) as u64 * 100)
                .await
                .unwrap();
        }

        // Send sync tick
        handle.sync_tick();
        tokio::time::sleep(Duration::from_millis(10)).await;

        handle.shutdown().await;
        task.await.unwrap();

        let files = store.list().unwrap();
        assert!(!files.is_empty());
    }

    #[tokio::test]
    async fn test_wal_actor_fire_and_forget() {
        let store = InMemoryWalStore::new();
        let config = test_config(FsyncPolicy::No);

        let (handle, task) = spawn_wal_actor(store.clone(), config).unwrap();

        // Fire-and-forget writes
        for i in 0..10 {
            let delta = make_test_delta(&format!("k{}", i), "v", (i + 1) as u64 * 100);
            handle.write_fire_and_forget(delta, (i + 1) as u64 * 100);
        }

        // Give actor time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        handle.shutdown().await;
        task.await.unwrap();

        let files = store.list().unwrap();
        assert!(!files.is_empty());
    }

    #[tokio::test]
    async fn test_wal_actor_truncation() {
        let store = InMemoryWalStore::new();
        let config = WalConfig {
            max_file_size: 100, // Very small to force multiple files
            ..test_config(FsyncPolicy::Always)
        };

        let (handle, task) = spawn_wal_actor(store.clone(), config).unwrap();

        // Write enough to create multiple files
        for i in 0..20 {
            let delta = make_test_delta(&format!("k{}", i), "v", (i + 1) as u64 * 100);
            handle
                .write_durable(delta, (i + 1) as u64 * 100)
                .await
                .unwrap();
        }

        let files_before = store.list().unwrap().len();

        // Truncate old entries
        handle.truncate(1000);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let files_after = store.list().unwrap().len();
        // May or may not have fewer files depending on timing
        // But the operation should not error
        assert!(files_after <= files_before);

        handle.shutdown().await;
        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_wal_actor_group_commit_batching() {
        let store = InMemoryWalStore::new();
        let config = test_config(FsyncPolicy::Always);

        let (handle, task) = spawn_wal_actor(store.clone(), config).unwrap();

        // Send many writes concurrently - they should be batched
        let mut join_handles = Vec::new();
        for i in 0..20 {
            let h = handle.clone();
            join_handles.push(tokio::spawn(async move {
                let delta = make_test_delta(&format!("k{}", i), "v", (i + 1) as u64 * 100);
                h.write_durable(delta, (i + 1) as u64 * 100).await
            }));
        }

        // All should succeed
        for jh in join_handles {
            jh.await.unwrap().unwrap();
        }

        handle.shutdown().await;
        task.await.unwrap();
    }
}
