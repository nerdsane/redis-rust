//! Streaming Persistence Coordinator
//!
//! Coordinates WriteBuffer and ManifestManager for complete persistence.
//! This is the main entry point for the streaming persistence system.
//!
//! ## Architecture (TigerStyle: explicit flow)
//!
//! ```text
//! Delta → StreamingPersistence::push()
//!              ↓
//!         WriteBuffer::push()
//!              ↓
//!         (on flush)
//!              ↓
//!         SegmentWriter → ObjectStore
//!              ↓
//!         ManifestManager::add_segment()
//! ```
//!
//! ## DST Compatibility
//!
//! All I/O through ObjectStore trait. Time through StreamingClock trait.
//! Both abstractions enable deterministic simulation testing.

use crate::replication::state::ReplicationDelta;
use crate::streaming::{
    Compression, Manifest, ManifestError, ManifestManager, ObjectStore, SegmentError,
    SegmentInfo, SegmentWriter, WriteBufferConfig, WriteBufferError,
    StreamingClock, StreamingTimestamp, ProductionClock,
};
use std::sync::Arc;

/// Error type for persistence operations
#[derive(Debug)]
pub enum PersistenceError {
    /// Write buffer error
    WriteBuffer(WriteBufferError),
    /// Manifest error
    Manifest(ManifestError),
    /// Segment error
    Segment(SegmentError),
    /// I/O error
    Io(std::io::Error),
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistenceError::WriteBuffer(e) => write!(f, "WriteBuffer error: {}", e),
            PersistenceError::Manifest(e) => write!(f, "Manifest error: {}", e),
            PersistenceError::Segment(e) => write!(f, "Segment error: {}", e),
            PersistenceError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for PersistenceError {}

impl From<WriteBufferError> for PersistenceError {
    fn from(e: WriteBufferError) -> Self {
        PersistenceError::WriteBuffer(e)
    }
}

impl From<ManifestError> for PersistenceError {
    fn from(e: ManifestError) -> Self {
        PersistenceError::Manifest(e)
    }
}

impl From<SegmentError> for PersistenceError {
    fn from(e: SegmentError) -> Self {
        PersistenceError::Segment(e)
    }
}

impl From<std::io::Error> for PersistenceError {
    fn from(e: std::io::Error) -> Self {
        PersistenceError::Io(e)
    }
}

/// Statistics for persistence operations
#[derive(Debug, Clone, Default)]
pub struct PersistenceStats {
    /// Total deltas pushed
    pub deltas_pushed: u64,
    /// Total segments written
    pub segments_written: u64,
    /// Total bytes written
    pub bytes_written: u64,
    /// Manifest updates
    pub manifest_updates: u64,
    /// Flush errors
    pub flush_errors: u64,
}

/// Result of a flush operation
#[derive(Debug)]
pub struct FlushResult {
    /// Segment info if a segment was written
    pub segment: Option<SegmentInfo>,
    /// Number of deltas flushed
    pub deltas_flushed: usize,
    /// Bytes written
    pub bytes_written: u64,
}

/// Streaming persistence coordinator
///
/// Manages WriteBuffer and ManifestManager together.
/// Generic over object store and clock for DST compatibility.
pub struct StreamingPersistence<S: ObjectStore + Clone + 'static, C: StreamingClock = ProductionClock> {
    store: Arc<S>,
    prefix: String,
    manifest_manager: ManifestManager<S>,
    config: WriteBufferConfig,
    compression: Compression,
    /// Current manifest (cached)
    manifest: Manifest,
    /// Buffered deltas waiting to be flushed
    buffer: Vec<ReplicationDelta>,
    /// Estimated buffer size in bytes
    buffer_size: usize,
    /// Clock for time operations (DST-compatible)
    clock: C,
    /// Last flush time
    last_flush: StreamingTimestamp,
    /// Statistics
    stats: PersistenceStats,
}

impl<S: ObjectStore + Clone + 'static> StreamingPersistence<S, ProductionClock> {
    /// Create a new streaming persistence coordinator with production clock
    pub async fn new(
        store: Arc<S>,
        prefix: String,
        replica_id: u64,
        config: WriteBufferConfig,
    ) -> Result<Self, PersistenceError> {
        Self::with_clock(store, prefix, replica_id, config, ProductionClock::new()).await
    }
}

impl<S: ObjectStore + Clone + 'static, C: StreamingClock> StreamingPersistence<S, C> {
    /// Create a new streaming persistence coordinator with custom clock
    ///
    /// Use this for DST testing with SimulatedClock.
    pub async fn with_clock(
        store: Arc<S>,
        prefix: String,
        replica_id: u64,
        config: WriteBufferConfig,
        clock: C,
    ) -> Result<Self, PersistenceError> {
        let manifest_manager = ManifestManager::new((*store).clone(), &prefix);
        let manifest = manifest_manager.load_or_create(replica_id).await?;

        let compression = if config.compression_enabled {
            #[cfg(feature = "compression")]
            {
                Compression::Zstd { level: 3 }
            }
            #[cfg(not(feature = "compression"))]
            {
                Compression::None
            }
        } else {
            Compression::None
        };

        let last_flush = clock.now();

        Ok(StreamingPersistence {
            store,
            prefix,
            manifest_manager,
            config,
            compression,
            manifest,
            buffer: Vec::new(),
            buffer_size: 0,
            clock,
            last_flush,
            stats: PersistenceStats::default(),
        })
    }

    /// Push a delta to the buffer
    ///
    /// Returns error if backpressure threshold is exceeded.
    pub fn push(&mut self, delta: ReplicationDelta) -> Result<(), PersistenceError> {
        // Check backpressure
        if self.buffer_size >= self.config.backpressure_threshold_bytes {
            return Err(PersistenceError::WriteBuffer(
                WriteBufferError::BackpressureExceeded {
                    pending_bytes: self.buffer_size,
                    threshold: self.config.backpressure_threshold_bytes,
                },
            ));
        }

        // Estimate delta size
        let delta_size = estimate_delta_size(&delta);

        self.buffer.push(delta);
        // TigerStyle: Use checked arithmetic for buffer size to catch corruption
        self.buffer_size = self.buffer_size.checked_add(delta_size)
            .expect("buffer_size overflow - indicates corrupted state or logic error");
        // TigerStyle: Use saturating arithmetic for stats (counters, non-critical)
        self.stats.deltas_pushed = self.stats.deltas_pushed.saturating_add(1);

        Ok(())
    }

    /// Check if buffer should be flushed
    pub fn should_flush(&self) -> bool {
        if self.buffer.is_empty() {
            return false;
        }

        // Size threshold
        if self.buffer_size >= self.config.max_size_bytes {
            return true;
        }

        // Count threshold
        if self.buffer.len() >= self.config.max_deltas {
            return true;
        }

        // Time threshold (DST-compatible via clock abstraction)
        if self.clock.has_elapsed(self.last_flush, self.config.flush_interval) {
            return true;
        }

        false
    }

    /// Flush buffer to object store and update manifest
    ///
    /// Returns info about the flush operation.
    pub async fn flush(&mut self) -> Result<FlushResult, PersistenceError> {
        if self.buffer.is_empty() {
            return Ok(FlushResult {
                segment: None,
                deltas_flushed: 0,
                bytes_written: 0,
            });
        }

        let deltas = std::mem::take(&mut self.buffer);
        let deltas_count = deltas.len();
        self.buffer_size = 0;
        self.last_flush = self.clock.now();

        // Calculate timestamps
        let min_timestamp = deltas
            .iter()
            .map(|d| d.value.timestamp.time)
            .min()
            .unwrap_or(0);
        let max_timestamp = deltas
            .iter()
            .map(|d| d.value.timestamp.time)
            .max()
            .unwrap_or(0);

        // IMPORTANT: Reload manifest from storage to get the latest next_segment_id.
        // This prevents ID collisions when compaction has allocated new segment IDs.
        // The trade-off is an extra I/O read per flush, but ensures correctness.
        self.manifest = self.manifest_manager.load_or_create(self.manifest.replica_id).await?;

        // Allocate segment ID
        let segment_id = self.manifest.allocate_segment_id();
        let segment_key = format!("{}/segments/segment-{:08}.seg", self.prefix, segment_id);

        // Write segment
        let mut writer = SegmentWriter::new(self.compression);
        for delta in &deltas {
            writer.write_delta(delta)?;
        }
        let data = writer.finish()?;
        let bytes_written = data.len() as u64;

        // Upload to object store
        self.store.put(&segment_key, &data).await?;

        // Create segment info
        let segment_info = SegmentInfo {
            id: segment_id,
            key: segment_key,
            record_count: deltas_count as u32,
            size_bytes: bytes_written,
            min_timestamp,
            max_timestamp,
        };

        // Update manifest
        self.manifest.add_segment(segment_info.clone());
        self.manifest_manager.save(&self.manifest).await?;

        // Update stats
        // TigerStyle: Use saturating arithmetic for stats (counters, non-critical)
        self.stats.segments_written = self.stats.segments_written.saturating_add(1);
        self.stats.bytes_written = self.stats.bytes_written.saturating_add(bytes_written);
        self.stats.manifest_updates = self.stats.manifest_updates.saturating_add(1);

        Ok(FlushResult {
            segment: Some(segment_info),
            deltas_flushed: deltas_count,
            bytes_written,
        })
    }

    /// Force flush regardless of thresholds
    pub async fn force_flush(&mut self) -> Result<FlushResult, PersistenceError> {
        self.flush().await
    }

    /// Get current manifest
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Get statistics
    pub fn stats(&self) -> &PersistenceStats {
        &self.stats
    }

    /// Get pending delta count
    pub fn pending_count(&self) -> usize {
        self.buffer.len()
    }

    /// Get pending buffer size in bytes
    pub fn pending_bytes(&self) -> usize {
        self.buffer_size
    }

    /// Get the manifest manager
    pub fn manifest_manager(&self) -> &ManifestManager<S> {
        &self.manifest_manager
    }

    /// Get the object store
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Get the prefix
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

/// Estimate the serialized size of a delta
fn estimate_delta_size(delta: &ReplicationDelta) -> usize {
    // Key length + value overhead + source_replica
    delta.key.len() + 64 + 8
}

/// Background worker for automatic flushing
pub struct PersistenceWorker<S: ObjectStore + Clone + 'static> {
    persistence: Arc<tokio::sync::Mutex<StreamingPersistence<S>>>,
    check_interval: std::time::Duration,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

/// Handle for controlling the persistence worker
pub struct PersistenceWorkerHandle {
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl PersistenceWorkerHandle {
    /// Signal shutdown
    pub fn shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl<S: ObjectStore + Clone + 'static> PersistenceWorker<S> {
    /// Create a new persistence worker
    pub fn new(
        persistence: Arc<tokio::sync::Mutex<StreamingPersistence<S>>>,
    ) -> (Self, PersistenceWorkerHandle) {
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = PersistenceWorkerHandle {
            shutdown: shutdown.clone(),
        };

        let worker = PersistenceWorker {
            persistence,
            check_interval: std::time::Duration::from_millis(50),
            shutdown,
        };

        (worker, handle)
    }

    /// Run the worker loop
    pub async fn run(self) {
        loop {
            if self
                .shutdown
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                // Final flush
                let mut p = self.persistence.lock().await;
                if let Err(e) = p.flush().await {
                    eprintln!("Error during final flush: {}", e);
                }
                break;
            }

            {
                let mut p = self.persistence.lock().await;
                if p.should_flush() {
                    if let Err(e) = p.flush().await {
                        eprintln!("Error flushing: {}", e);
                        // TigerStyle: Use saturating arithmetic for error counter
                        p.stats.flush_errors = p.stats.flush_errors.saturating_add(1);
                    }
                }
            }

            tokio::time::sleep(self.check_interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;
    use crate::replication::lattice::{LamportClock, ReplicaId};
    use crate::replication::state::ReplicatedValue;
    use crate::streaming::InMemoryObjectStore;

    fn make_delta(key: &str, value: &str, ts: u64) -> ReplicationDelta {
        let replica_id = ReplicaId::new(1);
        let clock = LamportClock {
            time: ts,
            replica_id,
        };
        let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }

    #[tokio::test]
    async fn test_persistence_push_and_flush() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let mut persistence =
            StreamingPersistence::new(store.clone(), "test".to_string(), 1, config)
                .await
                .unwrap();

        // Push some deltas
        persistence.push(make_delta("key1", "v1", 100)).unwrap();
        persistence.push(make_delta("key2", "v2", 200)).unwrap();

        assert_eq!(persistence.pending_count(), 2);

        // Flush
        let result = persistence.flush().await.unwrap();

        assert!(result.segment.is_some());
        assert_eq!(result.deltas_flushed, 2);
        assert_eq!(persistence.pending_count(), 0);

        // Manifest should have the segment
        assert_eq!(persistence.manifest().segments.len(), 1);
        assert_eq!(persistence.stats().segments_written, 1);
    }

    #[tokio::test]
    async fn test_persistence_manifest_persisted() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();

        // First instance
        {
            let mut persistence =
                StreamingPersistence::new(store.clone(), "test".to_string(), 1, config.clone())
                    .await
                    .unwrap();

            persistence.push(make_delta("key1", "v1", 100)).unwrap();
            persistence.flush().await.unwrap();
        }

        // Second instance should see the segment
        {
            let persistence =
                StreamingPersistence::new(store, "test".to_string(), 1, config)
                    .await
                    .unwrap();

            assert_eq!(persistence.manifest().segments.len(), 1);
        }
    }

    #[tokio::test]
    async fn test_persistence_should_flush_size() {
        let store = Arc::new(InMemoryObjectStore::new());
        let mut config = WriteBufferConfig::test();
        config.max_size_bytes = 100;
        config.max_deltas = 10000;
        config.flush_interval = std::time::Duration::from_secs(3600);

        let mut persistence =
            StreamingPersistence::new(store, "test".to_string(), 1, config)
                .await
                .unwrap();

        // Push until size threshold
        for i in 0..10 {
            persistence
                .push(make_delta(&format!("key{}", i), "some data", 100))
                .unwrap();
        }

        assert!(persistence.should_flush());
    }

    #[tokio::test]
    async fn test_persistence_should_flush_count() {
        let store = Arc::new(InMemoryObjectStore::new());
        let mut config = WriteBufferConfig::test();
        config.max_size_bytes = 1_000_000;
        config.max_deltas = 5;
        config.flush_interval = std::time::Duration::from_secs(3600);

        let mut persistence =
            StreamingPersistence::new(store, "test".to_string(), 1, config)
                .await
                .unwrap();

        for i in 0..5 {
            persistence.push(make_delta(&format!("k{}", i), "v", 100)).unwrap();
        }

        assert!(persistence.should_flush());
    }

    #[tokio::test]
    async fn test_persistence_worker() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let persistence = Arc::new(tokio::sync::Mutex::new(
            StreamingPersistence::new(store.clone(), "test".to_string(), 1, config)
                .await
                .unwrap(),
        ));

        // Push some deltas
        {
            let mut p = persistence.lock().await;
            p.push(make_delta("key1", "v1", 100)).unwrap();
        }

        let (worker, handle) = PersistenceWorker::new(persistence.clone());
        let worker_task = tokio::spawn(worker.run());

        // Wait for flush
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        handle.shutdown();
        worker_task.await.unwrap();

        // Should have flushed
        let p = persistence.lock().await;
        assert_eq!(p.manifest().segments.len(), 1);
    }

    // DST test with simulated store
    #[tokio::test]
    async fn test_persistence_with_simulated_store() {
        use crate::io::simulation::SimulatedRng;
        use crate::streaming::{SimulatedObjectStore, SimulatedStoreConfig};

        let inner = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(42);
        let store = Arc::new(SimulatedObjectStore::new(
            inner,
            rng,
            SimulatedStoreConfig::no_faults(),
        ));

        let config = WriteBufferConfig::test();
        let mut persistence =
            StreamingPersistence::new(store, "dst".to_string(), 1, config)
                .await
                .unwrap();

        persistence.push(make_delta("key1", "v1", 100)).unwrap();
        let result = persistence.flush().await.unwrap();

        assert!(result.segment.is_some());
        assert_eq!(result.deltas_flushed, 1);
    }

    // DST test with simulated clock - deterministic time-based flushing
    #[tokio::test]
    async fn test_persistence_with_simulated_clock() {
        use crate::streaming::SimulatedClock;

        let store = Arc::new(InMemoryObjectStore::new());
        let clock = SimulatedClock::new(1000);

        let mut config = WriteBufferConfig::test();
        config.flush_interval = std::time::Duration::from_millis(100);
        config.max_size_bytes = 1_000_000; // Disable size-based flush
        config.max_deltas = 10_000; // Disable count-based flush

        let mut persistence = StreamingPersistence::with_clock(
            store,
            "clock-test".to_string(),
            1,
            config,
            clock.clone(),
        )
        .await
        .unwrap();

        // Push a delta
        persistence.push(make_delta("key1", "v1", 100)).unwrap();

        // Time hasn't advanced, should not trigger time-based flush
        assert!(!persistence.should_flush());

        // Advance time by 50ms - still not enough
        clock.advance_ms(50);
        assert!(!persistence.should_flush());

        // Advance time to 100ms total - now should flush
        clock.advance_ms(50);
        assert!(persistence.should_flush());

        // Flush and verify
        let result = persistence.flush().await.unwrap();
        assert!(result.segment.is_some());
        assert_eq!(result.deltas_flushed, 1);

        // After flush, should_flush should be false again
        assert!(!persistence.should_flush());

        // Push another delta
        persistence.push(make_delta("key2", "v2", 200)).unwrap();

        // Need to wait another flush_interval
        assert!(!persistence.should_flush());
        clock.advance_ms(100);
        assert!(persistence.should_flush());
    }

    // DST test: deterministic replay with same seed produces same results
    #[tokio::test]
    async fn test_persistence_deterministic_replay() {
        use crate::io::simulation::SimulatedRng;
        use crate::streaming::{SimulatedClock, SimulatedObjectStore, SimulatedStoreConfig};

        async fn run_with_seed(seed: u64) -> (u64, u64) {
            let inner = InMemoryObjectStore::new();
            let rng = SimulatedRng::new(seed);
            let store = Arc::new(SimulatedObjectStore::new(
                inner,
                rng,
                SimulatedStoreConfig {
                    put_fail_prob: 0.1,
                    ..SimulatedStoreConfig::no_faults()
                },
            ));
            let clock = SimulatedClock::new(0);

            let config = WriteBufferConfig::test();
            let mut persistence = StreamingPersistence::with_clock(
                store,
                "replay-test".to_string(),
                1,
                config,
                clock.clone(),
            )
            .await
            .unwrap();

            let mut success_count = 0;
            let mut fail_count = 0;

            for i in 0..20 {
                persistence.push(make_delta(&format!("key{}", i), "value", i as u64)).unwrap();
                clock.advance_ms(100);

                match persistence.flush().await {
                    Ok(_) => success_count += 1,
                    Err(_) => fail_count += 1,
                }
            }

            (success_count, fail_count)
        }

        // Run twice with same seed
        let (s1, f1) = run_with_seed(42).await;
        let (s2, f2) = run_with_seed(42).await;

        assert_eq!(s1, s2, "Same seed should produce same success count");
        assert_eq!(f1, f2, "Same seed should produce same failure count");

        // Different seed should (likely) produce different results
        let (s3, _f3) = run_with_seed(999).await;
        // Note: This could theoretically be the same, but is very unlikely
        // We don't assert inequality because it's probabilistic
        println!("Seed 42: {} success, {} fail", s1, f1);
        println!("Seed 999: {} success", s3);
    }
}
