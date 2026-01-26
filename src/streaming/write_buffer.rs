//! Write Buffer for Streaming Persistence
//!
//! Buffers deltas in memory and flushes to object store periodically
//! or when size thresholds are reached.

use crate::replication::state::ReplicationDelta;
use crate::streaming::{Compression, ObjectStore, SegmentWriter, WriteBufferConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Error type for write buffer operations
#[derive(Debug)]
pub enum WriteBufferError {
    /// Buffer is full (backpressure)
    BackpressureExceeded {
        pending_bytes: usize,
        threshold: usize,
    },
    /// Serialization error
    Serialization(String),
    /// Object store error
    ObjectStore(std::io::Error),
    /// Segment error
    Segment(crate::streaming::SegmentError),
    /// Internal lock error (should never happen in practice)
    LockError(String),
}

impl std::fmt::Display for WriteBufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteBufferError::BackpressureExceeded {
                pending_bytes,
                threshold,
            } => {
                write!(
                    f,
                    "Backpressure exceeded: {} bytes pending, threshold {} bytes",
                    pending_bytes, threshold
                )
            }
            WriteBufferError::Serialization(msg) => write!(f, "Serialization error: {}", msg),
            WriteBufferError::ObjectStore(e) => write!(f, "Object store error: {}", e),
            WriteBufferError::Segment(e) => write!(f, "Segment error: {}", e),
            WriteBufferError::LockError(msg) => write!(f, "Lock error: {}", msg),
        }
    }
}

impl std::error::Error for WriteBufferError {}

impl From<crate::streaming::SegmentError> for WriteBufferError {
    fn from(e: crate::streaming::SegmentError) -> Self {
        WriteBufferError::Segment(e)
    }
}

impl From<std::io::Error> for WriteBufferError {
    fn from(e: std::io::Error) -> Self {
        WriteBufferError::ObjectStore(e)
    }
}

/// Statistics about write buffer state
#[derive(Debug, Clone, Default)]
pub struct WriteBufferStats {
    /// Current number of buffered deltas
    pub buffered_deltas: usize,
    /// Current buffer size in bytes (estimated)
    pub buffered_bytes: usize,
    /// Total deltas flushed since creation
    pub total_deltas_flushed: u64,
    /// Total segments written since creation
    pub total_segments_written: u64,
    /// Total bytes written since creation
    pub total_bytes_written: u64,
}

/// Inner state for the write buffer
struct WriteBufferInner {
    /// Buffered deltas waiting to be flushed
    deltas: Vec<ReplicationDelta>,
    /// Estimated size of buffered deltas
    estimated_bytes: usize,
    /// Last flush time
    last_flush: Instant,
    /// Segment counter for naming
    segment_counter: u64,
    /// Statistics
    stats: WriteBufferStats,
}

impl WriteBufferInner {
    fn new() -> Self {
        WriteBufferInner {
            deltas: Vec::new(),
            estimated_bytes: 0,
            last_flush: Instant::now(),
            segment_counter: 0,
            stats: WriteBufferStats::default(),
        }
    }

    /// TigerStyle: Verify all invariants hold
    ///
    /// # Invariants
    /// - stats.buffered_deltas == deltas.len()
    /// - stats.buffered_bytes == estimated_bytes
    /// - Empty buffer implies zero bytes
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        debug_assert_eq!(
            self.stats.buffered_deltas,
            self.deltas.len(),
            "Invariant violated: stats.buffered_deltas must equal deltas.len()"
        );
        debug_assert_eq!(
            self.stats.buffered_bytes, self.estimated_bytes,
            "Invariant violated: stats.buffered_bytes must equal estimated_bytes"
        );
        debug_assert!(
            !self.deltas.is_empty() || self.estimated_bytes == 0,
            "Invariant violated: empty buffer must have zero estimated_bytes"
        );
    }
}

/// Write buffer that accumulates deltas and flushes to object store
pub struct WriteBuffer<S: ObjectStore> {
    /// Configuration
    config: WriteBufferConfig,
    /// Object store to write segments to
    store: Arc<S>,
    /// Key prefix for segments
    prefix: String,
    /// Inner state protected by mutex
    inner: Mutex<WriteBufferInner>,
    /// Compression to use for segments
    compression: Compression,
}

impl<S: ObjectStore> WriteBuffer<S> {
    /// Create a new write buffer
    pub fn new(store: Arc<S>, prefix: String, config: WriteBufferConfig) -> Self {
        #[cfg(feature = "compression")]
        let compression = if config.compression_enabled {
            Compression::Zstd { level: 3 }
        } else {
            Compression::None
        };

        #[cfg(not(feature = "compression"))]
        let compression = Compression::None;

        WriteBuffer {
            config,
            store,
            prefix,
            inner: Mutex::new(WriteBufferInner::new()),
            compression,
        }
    }

    /// TigerStyle: Acquire lock with graceful poison recovery
    ///
    /// If a previous holder panicked, we still return the data since
    /// the WriteBufferInner state is still valid for our use case.
    #[inline]
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, WriteBufferInner> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            // Log the poison but recover the data - the buffer state is still usable
            eprintln!("WriteBuffer: recovering from poisoned lock");
            poisoned.into_inner()
        })
    }

    /// Push a delta to the buffer
    ///
    /// Returns error if backpressure threshold is exceeded
    pub fn push(&self, delta: ReplicationDelta) -> Result<(), WriteBufferError> {
        let mut inner = self.lock_inner();

        // Check backpressure before accepting
        if inner.estimated_bytes >= self.config.backpressure_threshold_bytes {
            return Err(WriteBufferError::BackpressureExceeded {
                pending_bytes: inner.estimated_bytes,
                threshold: self.config.backpressure_threshold_bytes,
            });
        }

        // Estimate size of this delta
        let delta_size = estimate_delta_size(&delta);

        inner.deltas.push(delta);
        inner.estimated_bytes += delta_size;
        inner.stats.buffered_deltas = inner.deltas.len();
        inner.stats.buffered_bytes = inner.estimated_bytes;

        #[cfg(debug_assertions)]
        inner.verify_invariants();

        Ok(())
    }

    /// Check if buffer should be flushed
    pub fn should_flush(&self) -> bool {
        let inner = self.lock_inner();

        // Empty buffer - no flush needed
        if inner.deltas.is_empty() {
            return false;
        }

        // Size threshold exceeded
        if inner.estimated_bytes >= self.config.max_size_bytes {
            return true;
        }

        // Delta count threshold exceeded
        if inner.deltas.len() >= self.config.max_deltas {
            return true;
        }

        // Time threshold exceeded
        if inner.last_flush.elapsed() >= self.config.flush_interval {
            return true;
        }

        false
    }

    /// Flush buffered deltas to object store
    ///
    /// Returns the segment key if anything was written, None if buffer was empty
    pub async fn flush(&self) -> Result<Option<String>, WriteBufferError> {
        // Take deltas from buffer
        let (deltas, segment_id) = {
            let mut inner = self.lock_inner();

            if inner.deltas.is_empty() {
                return Ok(None);
            }

            let deltas = std::mem::take(&mut inner.deltas);
            inner.estimated_bytes = 0;
            inner.last_flush = Instant::now();

            let segment_id = inner.segment_counter;
            inner.segment_counter += 1;

            inner.stats.buffered_deltas = 0;
            inner.stats.buffered_bytes = 0;
            inner.stats.total_deltas_flushed += deltas.len() as u64;

            #[cfg(debug_assertions)]
            inner.verify_invariants();

            (deltas, segment_id)
        };

        // Write segment
        let segment_key = format!("{}/segment-{:08}.seg", self.prefix, segment_id);

        let mut writer = SegmentWriter::new(self.compression);
        for delta in &deltas {
            writer.write_delta(delta)?;
        }
        let data = writer.finish()?;
        let data_len = data.len();

        // Write to object store
        self.store.put(&segment_key, &data).await?;

        // Update stats
        {
            let mut inner = self.lock_inner();
            inner.stats.total_segments_written += 1;
            inner.stats.total_bytes_written += data_len as u64;
        }

        Ok(Some(segment_key))
    }

    /// Get current statistics
    pub fn stats(&self) -> WriteBufferStats {
        let inner = self.lock_inner();
        inner.stats.clone()
    }

    /// Get number of pending deltas
    pub fn pending_count(&self) -> usize {
        let inner = self.lock_inner();
        inner.deltas.len()
    }

    /// Get estimated pending bytes
    pub fn pending_bytes(&self) -> usize {
        let inner = self.lock_inner();
        inner.estimated_bytes
    }
}

/// Estimate the serialized size of a delta
fn estimate_delta_size(delta: &ReplicationDelta) -> usize {
    // Key length + value overhead + source_replica
    // This is a rough estimate; actual serialized size may vary
    delta.key.len() + 64 + 8 // key + estimated value + replica id
}

/// Handle for controlling the flush worker
pub struct FlushWorkerHandle {
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl FlushWorkerHandle {
    /// Signal the flush worker to stop
    pub fn shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Background worker that periodically flushes the write buffer
pub struct FlushWorker<S: ObjectStore> {
    buffer: Arc<WriteBuffer<S>>,
    check_interval: Duration,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl<S: ObjectStore> FlushWorker<S> {
    /// Create a new flush worker
    pub fn new(buffer: Arc<WriteBuffer<S>>) -> (Self, FlushWorkerHandle) {
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = FlushWorkerHandle {
            shutdown: shutdown.clone(),
        };

        let worker = FlushWorker {
            buffer,
            check_interval: Duration::from_millis(50), // Check more frequently than flush interval
            shutdown,
        };

        (worker, handle)
    }

    /// Run the flush worker loop
    ///
    /// This should be spawned as a background task
    pub async fn run(self) {
        loop {
            if self.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                // Final flush before shutdown
                if let Err(e) = self.buffer.flush().await {
                    eprintln!("Error during final flush: {}", e);
                }
                break;
            }

            if self.buffer.should_flush() {
                if let Err(e) = self.buffer.flush().await {
                    eprintln!("Error flushing write buffer: {}", e);
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

    fn make_delta(key: &str, value: &str, replica: u64) -> ReplicationDelta {
        let replica_id = ReplicaId::new(replica);
        let clock = LamportClock {
            time: 1000,
            replica_id,
        };
        let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }

    #[test]
    fn test_write_buffer_push() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let buffer = WriteBuffer::new(store, "test".to_string(), config);

        let delta = make_delta("key1", "value1", 1);
        buffer.push(delta).unwrap();

        assert_eq!(buffer.pending_count(), 1);
        assert!(buffer.pending_bytes() > 0);
    }

    #[test]
    fn test_write_buffer_backpressure() {
        let store = Arc::new(InMemoryObjectStore::new());
        let mut config = WriteBufferConfig::test();
        config.backpressure_threshold_bytes = 100; // Very small threshold
        let buffer = WriteBuffer::new(store, "test".to_string(), config);

        // Fill buffer until backpressure
        let mut count = 0;
        loop {
            let delta = make_delta(&format!("key{}", count), "some value here", 1);
            match buffer.push(delta) {
                Ok(_) => count += 1,
                Err(WriteBufferError::BackpressureExceeded { .. }) => break,
                Err(e) => panic!("Unexpected error: {}", e),
            }
            if count > 1000 {
                panic!("Should have hit backpressure by now");
            }
        }

        assert!(count > 0, "Should have accepted at least one delta");
    }

    #[test]
    fn test_should_flush_empty() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let buffer = WriteBuffer::new(store, "test".to_string(), config);

        assert!(!buffer.should_flush());
    }

    #[test]
    fn test_should_flush_size_threshold() {
        let store = Arc::new(InMemoryObjectStore::new());
        let mut config = WriteBufferConfig::test();
        config.max_size_bytes = 100;
        config.max_deltas = 10000;
        config.flush_interval = Duration::from_secs(3600);
        let buffer = WriteBuffer::new(store, "test".to_string(), config);

        // Add deltas until size threshold
        for i in 0..10 {
            let delta = make_delta(&format!("key{}", i), "some value data", 1);
            buffer.push(delta).unwrap();
        }

        assert!(buffer.should_flush());
    }

    #[test]
    fn test_should_flush_delta_count() {
        let store = Arc::new(InMemoryObjectStore::new());
        let mut config = WriteBufferConfig::test();
        config.max_size_bytes = 1_000_000;
        config.max_deltas = 5;
        config.flush_interval = Duration::from_secs(3600);
        let buffer = WriteBuffer::new(store, "test".to_string(), config);

        for i in 0..5 {
            let delta = make_delta(&format!("key{}", i), "v", 1);
            buffer.push(delta).unwrap();
        }

        assert!(buffer.should_flush());
    }

    #[tokio::test]
    async fn test_flush_writes_segment() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let buffer = WriteBuffer::new(store.clone(), "segments".to_string(), config);

        // Add some deltas
        for i in 0..5 {
            let delta = make_delta(&format!("key{}", i), &format!("value{}", i), 1);
            buffer.push(delta).unwrap();
        }

        // Flush
        let segment_key = buffer.flush().await.unwrap();
        assert!(segment_key.is_some());

        let key = segment_key.unwrap();
        assert!(key.starts_with("segments/segment-"));
        assert!(key.ends_with(".seg"));

        // Verify segment was written
        assert!(store.exists(&key).await.unwrap());

        // Verify stats
        let stats = buffer.stats();
        assert_eq!(stats.total_deltas_flushed, 5);
        assert_eq!(stats.total_segments_written, 1);
        assert!(stats.total_bytes_written > 0);
        assert_eq!(stats.buffered_deltas, 0);
    }

    #[tokio::test]
    async fn test_flush_empty_returns_none() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let buffer = WriteBuffer::new(store, "test".to_string(), config);

        let result = buffer.flush().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_multiple_flushes_increment_counter() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let buffer = WriteBuffer::new(store.clone(), "seg".to_string(), config);

        for batch in 0..3 {
            for i in 0..2 {
                let delta = make_delta(&format!("batch{}key{}", batch, i), "v", 1);
                buffer.push(delta).unwrap();
            }
            buffer.flush().await.unwrap();
        }

        // Verify three segments were written with incrementing numbers
        assert!(store.exists("seg/segment-00000000.seg").await.unwrap());
        assert!(store.exists("seg/segment-00000001.seg").await.unwrap());
        assert!(store.exists("seg/segment-00000002.seg").await.unwrap());

        let stats = buffer.stats();
        assert_eq!(stats.total_segments_written, 3);
        assert_eq!(stats.total_deltas_flushed, 6);
    }

    #[tokio::test]
    async fn test_flush_worker_shutdown() {
        let store = Arc::new(InMemoryObjectStore::new());
        let config = WriteBufferConfig::test();
        let buffer = Arc::new(WriteBuffer::new(store.clone(), "test".to_string(), config));

        // Add a delta
        buffer.push(make_delta("key", "value", 1)).unwrap();

        let (worker, handle) = FlushWorker::new(buffer.clone());

        // Run worker in background
        let worker_task = tokio::spawn(worker.run());

        // Give it a moment
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Shutdown
        handle.shutdown();

        // Worker should complete
        tokio::time::timeout(Duration::from_secs(1), worker_task)
            .await
            .expect("Worker should complete within timeout")
            .expect("Worker task should not panic");

        // Final flush should have written the segment
        assert!(store.exists("test/segment-00000000.seg").await.unwrap());
    }
}
