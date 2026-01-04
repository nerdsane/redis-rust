//! Streaming Persistence Integration
//!
//! Provides integration functions for adding streaming persistence to a server.
//! Handles initialization, recovery, and background worker management.
//!
//! ## Usage (TigerStyle: explicit flow)
//!
//! ```text
//! 1. StreamingIntegration::new(config)
//! 2. integration.recover(state).await
//! 3. integration.start_workers(state)
//! 4. [server runs]
//! 5. integration.shutdown().await
//! ```

use crate::production::ReplicatedShardedState;
use crate::streaming::{
    Compactor, CompactionConfig, CompactionWorker, CompactionWorkerHandle,
    DeltaSinkSender, DeltaSinkReceiver, InMemoryObjectStore, LocalFsObjectStore,
    ManifestManager, ObjectStore, RecoveryError,
    RecoveryManager, RecoveryPhase, RecoveryStats,
    StreamingConfig, ObjectStoreType, StreamingPersistence,
    delta_sink_channel,
};
#[cfg(feature = "s3")]
use crate::streaming::S3ObjectStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{info, error};

/// Error type for integration operations
#[derive(Debug)]
pub enum IntegrationError {
    /// Configuration error
    Config(String),
    /// Recovery error
    Recovery(RecoveryError),
    /// I/O error
    Io(std::io::Error),
    /// Persistence error
    Persistence(String),
}

impl std::fmt::Display for IntegrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntegrationError::Config(msg) => write!(f, "Configuration error: {}", msg),
            IntegrationError::Recovery(e) => write!(f, "Recovery error: {}", e),
            IntegrationError::Io(e) => write!(f, "I/O error: {}", e),
            IntegrationError::Persistence(msg) => write!(f, "Persistence error: {}", msg),
        }
    }
}

impl std::error::Error for IntegrationError {}

impl From<RecoveryError> for IntegrationError {
    fn from(e: RecoveryError) -> Self {
        IntegrationError::Recovery(e)
    }
}

impl From<std::io::Error> for IntegrationError {
    fn from(e: std::io::Error) -> Self {
        IntegrationError::Io(e)
    }
}

/// Handle for controlling the delta sink bridge worker
struct DeltaSinkBridgeHandle {
    shutdown: Arc<AtomicBool>,
}

impl DeltaSinkBridgeHandle {
    fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Handles for background workers (actor-based architecture)
pub struct WorkerHandles {
    /// Actor handle for persistence operations
    actor_handle: PersistenceActorHandle,
    /// Actor task
    actor_task: JoinHandle<()>,
    /// Bridge handle (connects std::sync channel to actor)
    bridge_handle: DeltaSinkBridgeHandle,
    /// Bridge task
    bridge_task: JoinHandle<()>,
    /// Compaction worker handle (if enabled)
    compaction_handle: Option<CompactionWorkerHandle>,
    /// Compaction worker task (if enabled)
    compaction_task: Option<JoinHandle<()>>,
}

impl WorkerHandles {
    /// Initiate graceful shutdown of all workers
    pub async fn shutdown(self) {
        info!("Initiating streaming persistence shutdown");

        // 1. Stop the bridge (stops feeding deltas to actor)
        self.bridge_handle.shutdown();
        if let Err(e) = self.bridge_task.await {
            error!("Bridge task error: {}", e);
        }

        // 2. Shutdown actor gracefully (waits for final flush)
        self.actor_handle.shutdown().await;
        if let Err(e) = self.actor_task.await {
            error!("Persistence actor task error: {}", e);
        }

        // 3. Stop compaction worker if present
        if let Some(ref handle) = self.compaction_handle {
            handle.shutdown();
        }
        if let Some(task) = self.compaction_task {
            if let Err(e) = task.await {
                error!("Compaction worker task error: {}", e);
            }
        }

        info!("Streaming persistence shutdown complete");
    }
}

/// Streaming persistence integration helper
///
/// Provides a simple interface for integrating streaming persistence
/// into a server application.
pub struct StreamingIntegration<S: ObjectStore + Clone + 'static> {
    store: Arc<S>,
    config: StreamingConfig,
    replica_id: u64,
    prefix: String,
}

impl StreamingIntegration<InMemoryObjectStore> {
    /// Create integration with in-memory store (for testing)
    pub fn new_in_memory(config: StreamingConfig, replica_id: u64) -> Self {
        let store = Arc::new(InMemoryObjectStore::new());
        let prefix = config.prefix.clone();
        StreamingIntegration {
            store,
            config,
            replica_id,
            prefix,
        }
    }
}

impl StreamingIntegration<LocalFsObjectStore> {
    /// Create integration with local filesystem store
    pub fn new_local_fs(
        config: StreamingConfig,
        replica_id: u64,
    ) -> Result<Self, IntegrationError> {
        let path = config.local_path.clone().ok_or_else(|| {
            IntegrationError::Config("local_path required for LocalFs store".to_string())
        })?;

        let store = Arc::new(LocalFsObjectStore::new(path));
        let prefix = config.prefix.clone();

        Ok(StreamingIntegration {
            store,
            config,
            replica_id,
            prefix,
        })
    }
}

impl<S: ObjectStore + Clone + Send + Sync + 'static> StreamingIntegration<S> {
    /// Create integration with custom object store
    pub fn with_store(store: Arc<S>, config: StreamingConfig, replica_id: u64) -> Self {
        let prefix = config.prefix.clone();
        StreamingIntegration {
            store,
            config,
            replica_id,
            prefix,
        }
    }

    /// Perform recovery and apply state to the provided ReplicatedShardedState
    ///
    /// Returns recovery statistics.
    pub async fn recover(
        &self,
        state: &ReplicatedShardedState,
    ) -> Result<RecoveryStats, IntegrationError> {
        let recovery = RecoveryManager::new(
            (*self.store).clone(),
            &self.prefix,
            self.replica_id,
        );

        if !recovery.needs_recovery().await? {
            info!("No existing persistence data found, starting fresh");
            return Ok(RecoveryStats::default());
        }

        info!("Starting recovery from object store");

        let recovered = recovery
            .recover_with_progress(|progress| {
                match progress.phase {
                    RecoveryPhase::LoadingManifest => {
                        info!("Loading manifest...");
                    }
                    RecoveryPhase::LoadingCheckpoint => {
                        info!("Loading checkpoint...");
                    }
                    RecoveryPhase::LoadingSegments => {
                        info!(
                            "Loading segments ({}/{})...",
                            progress.segments_loaded, progress.segments_total
                        );
                    }
                    RecoveryPhase::Complete => {
                        info!(
                            "Recovery complete: {} segments, {} deltas, {} bytes",
                            progress.segments_loaded,
                            progress.deltas_replayed,
                            progress.bytes_read
                        );
                    }
                    _ => {}
                }
            })
            .await?;

        // Apply recovered state
        state.apply_recovered_state(recovered.checkpoint_state, recovered.deltas);

        info!(
            "Applied recovered state: {} keys",
            state.key_count()
        );

        Ok(recovered.stats)
    }

    /// Start background workers and wire up delta sink
    ///
    /// Returns handles for graceful shutdown and the delta sink sender
    /// that should be set on the ReplicatedShardedState.
    ///
    /// ## Architecture (Actor Pattern)
    ///
    /// ```text
    /// DeltaSinkSender ──► DeltaSinkReceiver ──► Bridge Task ──► PersistenceActor
    ///   (std::sync)         (std::sync)          (async)         (owns state)
    /// ```
    ///
    /// The Bridge task drains the std::sync channel and forwards to the async actor.
    /// The PersistenceActor owns StreamingPersistence exclusively (no shared mutex).
    pub async fn start_workers(
        &self,
    ) -> Result<(WorkerHandles, DeltaSinkSender), IntegrationError> {
        // Create delta sink channel (std::sync for fire-and-forget from command execution)
        let (sender, receiver) = delta_sink_channel();

        // Create StreamingPersistence (will be owned exclusively by actor)
        let persistence = StreamingPersistence::new(
            self.store.clone(),
            self.prefix.clone(),
            self.replica_id,
            self.config.write_buffer.clone(),
        )
        .await
        .map_err(|e| IntegrationError::Persistence(e.to_string()))?;

        // Spawn persistence actor (owns state, processes messages)
        let (actor_handle, actor_task) = spawn_persistence_actor(persistence);

        // Create bridge shutdown flag
        let bridge_shutdown = Arc::new(AtomicBool::new(false));
        let bridge_handle = DeltaSinkBridgeHandle {
            shutdown: bridge_shutdown.clone(),
        };

        // Spawn bridge task (connects std::sync channel to async actor)
        let actor_handle_clone = actor_handle.clone();
        let flush_interval = self.config.write_buffer.flush_interval;
        let bridge_task = tokio::spawn(async move {
            run_delta_sink_bridge(receiver, actor_handle_clone, bridge_shutdown, flush_interval).await;
        });

        info!(
            "Started persistence actor (flush interval: {:?})",
            self.config.write_buffer.flush_interval
        );

        // Create manifest manager for compaction (separate instance)
        let manifest_manager = ManifestManager::new((*self.store).clone(), &self.prefix);

        // Start compaction worker if configured
        let (compaction_handle, compaction_task) = if self.config.compaction.max_segments > 0 {
            let compaction_config = CompactionConfig {
                target_segment_size: self.config.compaction.target_segment_size,
                max_segments: self.config.compaction.max_segments,
                min_segments_to_compact: self.config.compaction.min_segments_to_compact,
                max_segments_per_compaction: self.config.compaction.max_segments_per_compaction,
                tombstone_ttl: self.config.compaction.tombstone_ttl,
                compression_enabled: self.config.compaction.compression_enabled,
            };

            let compactor = Compactor::new(
                self.store.clone(),
                self.prefix.clone(),
                manifest_manager,
                compaction_config,
            );

            let check_interval = Duration::from_secs(60); // Check every minute
            let (worker, handle) = CompactionWorker::new(compactor, check_interval);
            let task = tokio::spawn(worker.run());

            info!(
                "Started compaction worker (max segments: {})",
                self.config.compaction.max_segments
            );
            (Some(handle), Some(task))
        } else {
            (None, None)
        };

        let handles = WorkerHandles {
            actor_handle,
            actor_task,
            bridge_handle,
            bridge_task,
            compaction_handle,
            compaction_task,
        };

        Ok((handles, sender))
    }

    /// Get the object store
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Get the configuration
    pub fn config(&self) -> &StreamingConfig {
        &self.config
    }
}

/// Create streaming integration from configuration
///
/// Factory function that creates the appropriate integration based on config.
pub async fn create_integration(
    config: StreamingConfig,
    replica_id: u64,
) -> Result<Box<dyn StreamingIntegrationTrait>, IntegrationError> {
    match config.store_type {
        ObjectStoreType::InMemory => {
            let integration = StreamingIntegration::new_in_memory(config, replica_id);
            Ok(Box::new(StreamingIntegrationWrapper::InMemory(integration)))
        }
        ObjectStoreType::LocalFs => {
            let integration = StreamingIntegration::new_local_fs(config, replica_id)?;
            Ok(Box::new(StreamingIntegrationWrapper::LocalFs(integration)))
        }
        #[cfg(feature = "s3")]
        ObjectStoreType::S3 => {
            let s3_config = config.s3.clone().ok_or_else(|| {
                IntegrationError::Config("S3 configuration required for S3 store type".to_string())
            })?;
            let store = S3ObjectStore::new(s3_config).await.map_err(|e| {
                IntegrationError::Config(format!("Failed to create S3 store: {}", e))
            })?;
            let integration = StreamingIntegration::with_store(Arc::new(store), config, replica_id);
            Ok(Box::new(StreamingIntegrationWrapper::S3(integration)))
        }
    }
}

/// Trait for type-erased streaming integration
pub trait StreamingIntegrationTrait: Send + Sync {
    /// Perform recovery
    fn recover<'a>(
        &'a self,
        state: &'a ReplicatedShardedState,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<RecoveryStats, IntegrationError>> + Send + 'a>>;

    /// Start workers
    fn start_workers<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(WorkerHandles, DeltaSinkSender), IntegrationError>> + Send + 'a>>;
}

/// Wrapper enum for type erasure
enum StreamingIntegrationWrapper {
    InMemory(StreamingIntegration<InMemoryObjectStore>),
    LocalFs(StreamingIntegration<LocalFsObjectStore>),
    #[cfg(feature = "s3")]
    S3(StreamingIntegration<S3ObjectStore>),
}

impl StreamingIntegrationTrait for StreamingIntegrationWrapper {
    fn recover<'a>(
        &'a self,
        state: &'a ReplicatedShardedState,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<RecoveryStats, IntegrationError>> + Send + 'a>> {
        match self {
            StreamingIntegrationWrapper::InMemory(i) => Box::pin(i.recover(state)),
            StreamingIntegrationWrapper::LocalFs(i) => Box::pin(i.recover(state)),
            #[cfg(feature = "s3")]
            StreamingIntegrationWrapper::S3(i) => Box::pin(i.recover(state)),
        }
    }

    fn start_workers<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(WorkerHandles, DeltaSinkSender), IntegrationError>> + Send + 'a>> {
        match self {
            StreamingIntegrationWrapper::InMemory(i) => Box::pin(i.start_workers()),
            StreamingIntegrationWrapper::LocalFs(i) => Box::pin(i.start_workers()),
            #[cfg(feature = "s3")]
            StreamingIntegrationWrapper::S3(i) => Box::pin(i.start_workers()),
        }
    }
}

// ============================================================================
// Persistence Actor (follows actor pattern from sharded_actor.rs)
// ============================================================================

use crate::replication::ReplicationDelta;
use tokio::sync::{mpsc, oneshot};

/// Messages for the persistence actor
#[derive(Debug)]
pub enum PersistenceMessage {
    /// Push a delta to the buffer
    PushDelta(ReplicationDelta),
    /// Push multiple deltas (batch)
    PushDeltas(Vec<ReplicationDelta>),
    /// Force a flush
    Flush {
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Check and flush if needed (periodic tick)
    Tick,
    /// Shutdown the actor
    Shutdown {
        response_tx: oneshot::Sender<()>,
    },
}

/// Actor that owns StreamingPersistence exclusively
struct PersistenceActor<S: ObjectStore + Clone + 'static> {
    persistence: StreamingPersistence<S>,
    rx: mpsc::UnboundedReceiver<PersistenceMessage>,
}

impl<S: ObjectStore + Clone + Send + Sync + 'static> PersistenceActor<S> {
    fn new(persistence: StreamingPersistence<S>, rx: mpsc::UnboundedReceiver<PersistenceMessage>) -> Self {
        PersistenceActor { persistence, rx }
    }

    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                PersistenceMessage::PushDelta(delta) => {
                    if let Err(e) = self.persistence.push(delta) {
                        error!("Failed to push delta: {}", e);
                    }
                    // Auto-flush if needed
                    if self.persistence.should_flush() {
                        if let Err(e) = self.persistence.flush().await {
                            error!("Failed to flush: {}", e);
                        }
                    }
                }
                PersistenceMessage::PushDeltas(deltas) => {
                    for delta in deltas {
                        if let Err(e) = self.persistence.push(delta) {
                            error!("Failed to push delta: {}", e);
                            break;
                        }
                    }
                    // Auto-flush if needed
                    if self.persistence.should_flush() {
                        if let Err(e) = self.persistence.flush().await {
                            error!("Failed to flush: {}", e);
                        }
                    }
                }
                PersistenceMessage::Flush { response_tx } => {
                    let result = self.persistence.flush().await
                        .map(|_| ())  // Discard FlushResult, just return ()
                        .map_err(|e| e.to_string());
                    let _ = response_tx.send(result);
                }
                PersistenceMessage::Tick => {
                    if self.persistence.should_flush() {
                        if let Err(e) = self.persistence.flush().await {
                            error!("Failed periodic flush: {}", e);
                        }
                    }
                }
                PersistenceMessage::Shutdown { response_tx } => {
                    // Final flush before shutdown
                    info!("Persistence actor shutting down, performing final flush...");
                    if let Err(e) = self.persistence.flush().await {
                        error!("Failed final flush: {}", e);
                    } else {
                        info!("Final flush complete");
                    }
                    let _ = response_tx.send(());
                    break;
                }
            }
        }
    }
}

/// Handle for communicating with the persistence actor
#[derive(Clone)]
pub struct PersistenceActorHandle {
    tx: mpsc::UnboundedSender<PersistenceMessage>,
}

impl PersistenceActorHandle {
    /// Push a delta (fire-and-forget)
    #[inline]
    pub fn push_delta(&self, delta: ReplicationDelta) {
        let _ = self.tx.send(PersistenceMessage::PushDelta(delta));
    }

    /// Push multiple deltas (fire-and-forget)
    #[inline]
    pub fn push_deltas(&self, deltas: Vec<ReplicationDelta>) {
        if !deltas.is_empty() {
            let _ = self.tx.send(PersistenceMessage::PushDeltas(deltas));
        }
    }

    /// Force a flush and wait for completion
    pub async fn flush(&self) -> Result<(), String> {
        let (response_tx, response_rx) = oneshot::channel();
        if self.tx.send(PersistenceMessage::Flush { response_tx }).is_err() {
            return Err("Persistence actor unavailable".to_string());
        }
        response_rx.await.unwrap_or(Err("Response channel dropped".to_string()))
    }

    /// Send periodic tick (fire-and-forget)
    #[inline]
    pub fn tick(&self) {
        let _ = self.tx.send(PersistenceMessage::Tick);
    }

    /// Shutdown the actor gracefully
    pub async fn shutdown(&self) {
        let (response_tx, response_rx) = oneshot::channel();
        if self.tx.send(PersistenceMessage::Shutdown { response_tx }).is_ok() {
            let _ = response_rx.await;
        }
    }
}

/// Spawn the persistence actor and return a handle
fn spawn_persistence_actor<S: ObjectStore + Clone + Send + Sync + 'static>(
    persistence: StreamingPersistence<S>,
) -> (PersistenceActorHandle, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let actor = PersistenceActor::new(persistence, rx);
    let task = tokio::spawn(actor.run());
    (PersistenceActorHandle { tx }, task)
}

/// Background task that bridges DeltaSinkReceiver to PersistenceActor
///
/// This drains the std::sync channel and forwards to the async actor.
/// Uses tokio::task::yield_now() to avoid blocking the runtime.
async fn run_delta_sink_bridge(
    receiver: DeltaSinkReceiver,
    actor_handle: PersistenceActorHandle,
    shutdown: Arc<AtomicBool>,
    flush_interval: Duration,
) {
    let mut last_tick = std::time::Instant::now();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        // Try to drain available deltas (non-blocking)
        let deltas = receiver.drain();
        if !deltas.is_empty() {
            actor_handle.push_deltas(deltas);
        } else {
            // No deltas available - yield and sleep briefly
            // This avoids blocking the async runtime
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Periodic tick for time-based flush
        if last_tick.elapsed() >= flush_interval {
            actor_handle.tick();
            last_tick = std::time::Instant::now();
        }
    }

    // Drain remaining deltas on shutdown
    let remaining = receiver.drain();
    if !remaining.is_empty() {
        actor_handle.push_deltas(remaining);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::{ReplicationConfig, ConsistencyLevel};

    #[tokio::test]
    async fn test_integration_in_memory() {
        let config = StreamingConfig::test();
        let integration = StreamingIntegration::new_in_memory(config, 1);

        let repl_config = ReplicationConfig {
            enabled: true,
            replica_id: 1,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 100,
            peers: vec![],
            replication_factor: 3,
            partitioned_mode: false,
            selective_gossip: false,
            virtual_nodes_per_physical: 150,
        };
        let state = ReplicatedShardedState::new(repl_config);

        // Recovery should succeed with no data
        let stats = integration.recover(&state).await.unwrap();
        assert_eq!(stats.segments_loaded, 0);

        // Start workers
        let (handles, _sender) = integration.start_workers().await.unwrap();

        // Graceful shutdown
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn test_integration_with_data() {
        use crate::redis::SDS;

        let config = StreamingConfig::test();
        let integration = StreamingIntegration::new_in_memory(config, 1);

        let repl_config = ReplicationConfig {
            enabled: true,
            replica_id: 1,
            consistency_level: ConsistencyLevel::Eventual,
            gossip_interval_ms: 100,
            peers: vec![],
            replication_factor: 3,
            partitioned_mode: false,
            selective_gossip: false,
            virtual_nodes_per_physical: 150,
        };
        let mut state = ReplicatedShardedState::new(repl_config.clone());

        // Start workers
        let (handles, sender) = integration.start_workers().await.unwrap();
        state.set_delta_sink(sender.clone());

        // Execute some commands
        state.execute(crate::redis::Command::Set(
            "key1".to_string(),
            SDS::from_str("value1"),
        ));
        state.execute(crate::redis::Command::Set(
            "key2".to_string(),
            SDS::from_str("value2"),
        ));

        // Give persistence time to flush
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Drop the sender to close the channel before shutdown
        drop(sender);
        state.clear_delta_sink();

        // Shutdown
        handles.shutdown().await;
    }
}
