//! Segment Compaction for Streaming Persistence
//!
//! Merges multiple small segments into larger ones and removes stale tombstones.
//! Compaction reduces read amplification during recovery and reclaims space
//! from deleted keys.
//!
//! ## Algorithm (TigerStyle: explicit steps)
//!
//! 1. Select oldest/smallest segments for compaction
//! 2. Read all deltas into HashMap (key -> latest delta)
//! 3. Remove tombstones older than TTL
//! 4. Write new compacted segment
//! 5. Atomic manifest update (add new, remove old)
//! 6. Delete old segments (best effort)
//!
//! ## DST Compatibility
//!
//! All I/O through ObjectStore trait. Deterministic segment selection
//! based on segment ID ordering.

use crate::io::{ProductionTimeSource, TimeSource};
use crate::replication::state::ReplicationDelta;
use crate::streaming::{
    Compression, Manifest, ManifestError, ManifestManager, ObjectStore, SegmentError,
    SegmentInfo, SegmentReader, SegmentWriter,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for compaction operations
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Target size for compacted segments (bytes)
    pub target_segment_size: usize,
    /// Maximum number of segments before compaction triggers
    pub max_segments: usize,
    /// Minimum number of segments to compact together
    pub min_segments_to_compact: usize,
    /// Maximum number of segments to compact in one pass
    pub max_segments_per_compaction: usize,
    /// Time-to-live for tombstones (deleted keys)
    pub tombstone_ttl: Duration,
    /// Enable compression for compacted segments
    pub compression_enabled: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        CompactionConfig {
            target_segment_size: 64 * 1024 * 1024, // 64MB
            max_segments: 100,
            min_segments_to_compact: 4,
            max_segments_per_compaction: 10,
            tombstone_ttl: Duration::from_secs(24 * 3600), // 24 hours
            compression_enabled: true,
        }
    }
}

impl CompactionConfig {
    /// Configuration for tests (smaller sizes, shorter TTL)
    pub fn test() -> Self {
        CompactionConfig {
            target_segment_size: 1024, // 1KB
            max_segments: 10,
            min_segments_to_compact: 2,
            max_segments_per_compaction: 5,
            tombstone_ttl: Duration::from_millis(100),
            compression_enabled: false,
        }
    }
}

/// Error type for compaction operations
#[derive(Debug)]
pub enum CompactionError {
    /// Manifest error
    Manifest(ManifestError),
    /// Segment error
    Segment(SegmentError),
    /// I/O error
    Io(std::io::Error),
    /// No segments to compact
    NothingToCompact,
}

impl std::fmt::Display for CompactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactionError::Manifest(e) => write!(f, "Manifest error: {}", e),
            CompactionError::Segment(e) => write!(f, "Segment error: {}", e),
            CompactionError::Io(e) => write!(f, "I/O error: {}", e),
            CompactionError::NothingToCompact => write!(f, "No segments to compact"),
        }
    }
}

impl std::error::Error for CompactionError {}

impl From<ManifestError> for CompactionError {
    fn from(e: ManifestError) -> Self {
        CompactionError::Manifest(e)
    }
}

impl From<SegmentError> for CompactionError {
    fn from(e: SegmentError) -> Self {
        CompactionError::Segment(e)
    }
}

impl From<std::io::Error> for CompactionError {
    fn from(e: std::io::Error) -> Self {
        CompactionError::Io(e)
    }
}

/// Result of a compaction operation
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Segments that were compacted (removed)
    pub segments_removed: Vec<SegmentInfo>,
    /// New compacted segment (if any)
    pub segment_created: Option<SegmentInfo>,
    /// Number of deltas before compaction
    pub deltas_before: u64,
    /// Number of deltas after compaction (after deduplication)
    pub deltas_after: u64,
    /// Number of tombstones removed due to TTL
    pub tombstones_removed: u64,
    /// Bytes reclaimed
    pub bytes_reclaimed: u64,
}

/// Statistics for compaction
#[derive(Debug, Clone, Default)]
pub struct CompactionStats {
    /// Total compactions performed
    pub compactions_performed: u64,
    /// Total segments removed
    pub segments_removed: u64,
    /// Total segments created
    pub segments_created: u64,
    /// Total bytes reclaimed
    pub bytes_reclaimed: u64,
    /// Total tombstones removed
    pub tombstones_removed: u64,
}

/// Compactor manages segment compaction
///
/// Generic over `T: TimeSource` for zero-cost abstraction:
/// - Production: `ProductionTimeSource` (ZST, compiles to syscall)
/// - Simulation: `SimulatedTimeSource` (virtual clock)
pub struct Compactor<S: ObjectStore + Clone + 'static, T: TimeSource = ProductionTimeSource> {
    store: Arc<S>,
    prefix: String,
    manifest_manager: ManifestManager<S>,
    config: CompactionConfig,
    stats: CompactionStats,
    time_source: T,
}

/// Production-specific constructors (use ProductionTimeSource)
impl<S: ObjectStore + Clone + 'static> Compactor<S, ProductionTimeSource> {
    /// Create a new compactor with production time source
    pub fn new(
        store: Arc<S>,
        prefix: String,
        manifest_manager: ManifestManager<S>,
        config: CompactionConfig,
    ) -> Self {
        Self::with_time_source(store, prefix, manifest_manager, config, ProductionTimeSource::new())
    }
}

/// Generic implementation that works with any TimeSource
impl<S: ObjectStore + Clone + 'static, T: TimeSource> Compactor<S, T> {
    /// Create a new compactor with custom time source
    ///
    /// This is the main constructor - all other constructors delegate to this.
    pub fn with_time_source(
        store: Arc<S>,
        prefix: String,
        manifest_manager: ManifestManager<S>,
        config: CompactionConfig,
        time_source: T,
    ) -> Self {
        Compactor {
            store,
            prefix,
            manifest_manager,
            config,
            stats: CompactionStats::default(),
            time_source,
        }
    }

    /// Check if compaction is needed
    pub async fn needs_compaction(&self) -> Result<bool, CompactionError> {
        let manifest = self.manifest_manager.load_or_create(0).await?;
        Ok(manifest.segments.len() >= self.config.max_segments)
    }

    /// Select segments for compaction
    ///
    /// Strategy: Select the oldest (lowest ID) small segments first.
    /// This prioritizes compacting older data which is less likely to change.
    fn select_segments_to_compact<'a>(&self, manifest: &'a Manifest) -> Vec<&'a SegmentInfo> {
        let mut candidates: Vec<&SegmentInfo> = manifest
            .segments
            .iter()
            .filter(|s| s.size_bytes < self.config.target_segment_size as u64)
            .collect();

        // Sort by ID (oldest first) for deterministic selection
        candidates.sort_by_key(|s| s.id);

        // Take up to max_segments_per_compaction
        candidates
            .into_iter()
            .take(self.config.max_segments_per_compaction)
            .collect()
    }

    /// Perform compaction
    ///
    /// Merges selected segments into a new segment, deduplicating keys
    /// and removing expired tombstones.
    pub async fn compact(&mut self) -> Result<CompactionResult, CompactionError> {
        // Load manifest
        let manifest = self.manifest_manager.load_or_create(0).await?;

        // Select segments to compact
        let segments_to_compact = self.select_segments_to_compact(&manifest);

        if segments_to_compact.len() < self.config.min_segments_to_compact {
            return Err(CompactionError::NothingToCompact);
        }

        // Calculate current timestamp for tombstone TTL
        // Uses TimeSource for zero-cost abstraction (syscall in production, virtual clock in simulation)
        let current_time = self.time_source.now_millis();
        let tombstone_cutoff = current_time.saturating_sub(self.config.tombstone_ttl.as_millis() as u64);

        // Load all deltas from selected segments, handling missing files gracefully
        let mut deltas_before = 0u64;
        let mut key_to_delta: HashMap<String, ReplicationDelta> = HashMap::new();
        let mut bytes_before = 0u64;
        let mut actually_compacted: Vec<&SegmentInfo> = Vec::new();
        let mut missing_segments: Vec<String> = Vec::new();

        for segment_info in &segments_to_compact {
            match self.store.get(&segment_info.key).await {
                Ok(data) => {
                    bytes_before += data.len() as u64;

                    let reader = match SegmentReader::open(&data) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("Failed to open segment {}: {}", segment_info.key, e);
                            continue;
                        }
                    };

                    if let Err(e) = reader.validate() {
                        eprintln!("Invalid segment {}: {}", segment_info.key, e);
                        continue;
                    }

                    match reader.deltas() {
                        Ok(deltas_iter) => {
                            for delta_result in deltas_iter {
                                match delta_result {
                                    Ok(delta) => {
                                        deltas_before += 1;

                                        // Keep latest delta for each key
                                        let should_insert = match key_to_delta.get(&delta.key) {
                                            Some(existing) => delta.value.timestamp.time > existing.value.timestamp.time,
                                            None => true,
                                        };

                                        if should_insert {
                                            key_to_delta.insert(delta.key.clone(), delta);
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to read delta from {}: {}", segment_info.key, e);
                                    }
                                }
                            }
                            actually_compacted.push(segment_info);
                        }
                        Err(e) => {
                            eprintln!("Failed to read deltas from {}: {}", segment_info.key, e);
                        }
                    }
                }
                Err(e) => {
                    // Segment missing (concurrent compaction or crash), mark for manifest cleanup
                    eprintln!("Segment {} missing: {}", segment_info.key, e);
                    missing_segments.push(segment_info.key.clone());
                    actually_compacted.push(segment_info);
                }
            }
        }

        // Clean up manifest if only missing segments found
        if !missing_segments.is_empty() && key_to_delta.is_empty() && deltas_before == 0 {
            let segments_removed: Vec<SegmentInfo> = actually_compacted
                .iter()
                .map(|s| (*s).clone())
                .collect();

            let mut new_manifest = manifest.clone();
            let segment_ids: Vec<u64> = segments_removed.iter().map(|s| s.id).collect();
            new_manifest.segments.retain(|s| !segment_ids.contains(&s.id));
            new_manifest.version += 1;
            self.manifest_manager.save(&new_manifest).await?;

            self.stats.compactions_performed += 1;
            self.stats.segments_removed += segments_removed.len() as u64;

            return Ok(CompactionResult {
                segments_removed,
                segment_created: None,
                deltas_before: 0,
                deltas_after: 0,
                tombstones_removed: 0,
                bytes_reclaimed: 0,
            });
        }

        // Check minimum segment requirement
        if actually_compacted.len() < self.config.min_segments_to_compact {
            return Err(CompactionError::NothingToCompact);
        }

        // Remove expired tombstones
        let mut tombstones_removed = 0u64;
        key_to_delta.retain(|_key, delta| {
            if delta.value.is_tombstone() {
                // Check if tombstone is older than TTL
                // Convert Lamport time to approximate wall clock (assuming ~1 tick per ms)
                let delta_time = delta.value.timestamp.time;
                if delta_time < tombstone_cutoff {
                    tombstones_removed += 1;
                    return false;
                }
            }
            true
        });

        let deltas_after = key_to_delta.len() as u64;

        // If nothing remains, just remove the segments
        if key_to_delta.is_empty() {
            let segments_removed: Vec<SegmentInfo> = actually_compacted
                .iter()
                .map(|s| (*s).clone())
                .collect();

            // Update manifest - remove old segments
            let mut new_manifest = manifest.clone();
            let segment_ids: Vec<u64> = segments_removed.iter().map(|s| s.id).collect();
            new_manifest.segments.retain(|s| !segment_ids.contains(&s.id));
            new_manifest.version += 1;
            self.manifest_manager.save(&new_manifest).await?;

            // Delete old segment files
            for segment in &segments_removed {
                let _ = self.store.delete(&segment.key).await;
            }

            // Update stats
            self.stats.compactions_performed += 1;
            self.stats.segments_removed += segments_removed.len() as u64;
            self.stats.bytes_reclaimed += bytes_before;
            self.stats.tombstones_removed += tombstones_removed;

            return Ok(CompactionResult {
                segments_removed,
                segment_created: None,
                deltas_before,
                deltas_after: 0,
                tombstones_removed,
                bytes_reclaimed: bytes_before,
            });
        }

        // Write new compacted segment
        let compression = if self.config.compression_enabled {
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

        let mut writer = SegmentWriter::new(compression);

        // Sort deltas by timestamp for deterministic output
        let mut deltas: Vec<ReplicationDelta> = key_to_delta.into_values().collect();
        deltas.sort_by_key(|d| d.value.timestamp.time);

        let mut min_timestamp = u64::MAX;
        let mut max_timestamp = 0u64;

        for delta in &deltas {
            writer.write_delta(delta)?;
            min_timestamp = min_timestamp.min(delta.value.timestamp.time);
            max_timestamp = max_timestamp.max(delta.value.timestamp.time);
        }

        let segment_data = writer.finish()?;
        let bytes_after = segment_data.len() as u64;

        // Generate new segment ID
        let new_segment_id = manifest.next_segment_id;
        let new_segment_key = format!(
            "{}/segments/segment-{:08}.seg",
            self.prefix, new_segment_id
        );

        // Upload new segment
        self.store.put(&new_segment_key, &segment_data).await?;

        let new_segment = SegmentInfo {
            id: new_segment_id,
            key: new_segment_key,
            record_count: deltas.len() as u32,
            size_bytes: bytes_after,
            min_timestamp,
            max_timestamp,
        };

        // Atomic manifest update
        let segments_removed: Vec<SegmentInfo> = actually_compacted
            .iter()
            .map(|s| (*s).clone())
            .collect();
        let segment_ids: Vec<u64> = segments_removed.iter().map(|s| s.id).collect();

        let mut new_manifest = manifest.clone();
        new_manifest.segments.retain(|s| !segment_ids.contains(&s.id));
        new_manifest.add_segment(new_segment.clone());
        new_manifest.next_segment_id = new_segment_id + 1;
        self.manifest_manager.save(&new_manifest).await?;

        // Delete old segment files (best effort, skip already-missing ones)
        for segment in &segments_removed {
            let _ = self.store.delete(&segment.key).await;
        }

        // Update stats
        let bytes_reclaimed = bytes_before.saturating_sub(bytes_after);
        self.stats.compactions_performed += 1;
        self.stats.segments_removed += segments_removed.len() as u64;
        self.stats.segments_created += 1;
        self.stats.bytes_reclaimed += bytes_reclaimed;
        self.stats.tombstones_removed += tombstones_removed;

        Ok(CompactionResult {
            segments_removed,
            segment_created: Some(new_segment),
            deltas_before,
            deltas_after,
            tombstones_removed,
            bytes_reclaimed,
        })
    }

    /// Compact if needed
    ///
    /// Performs compaction only if segment count exceeds threshold.
    pub async fn compact_if_needed(&mut self) -> Result<Option<CompactionResult>, CompactionError> {
        if self.needs_compaction().await? {
            match self.compact().await {
                Ok(result) => Ok(Some(result)),
                Err(CompactionError::NothingToCompact) => Ok(None),
                Err(e) => Err(e),
            }
        } else {
            Ok(None)
        }
    }

    /// Get compaction statistics
    pub fn stats(&self) -> &CompactionStats {
        &self.stats
    }

    /// Get configuration
    pub fn config(&self) -> &CompactionConfig {
        &self.config
    }
}

/// Background compaction worker
///
/// Generic over time source for DST compatibility.
pub struct CompactionWorker<S: ObjectStore + Clone + 'static, T: TimeSource = ProductionTimeSource> {
    compactor: Compactor<S, T>,
    check_interval: Duration,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

/// Handle for controlling the compaction worker
pub struct CompactionWorkerHandle {
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl CompactionWorkerHandle {
    /// Signal shutdown
    pub fn shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl<S: ObjectStore + Clone + 'static, T: TimeSource> CompactionWorker<S, T> {
    /// Create a new compaction worker
    pub fn new(compactor: Compactor<S, T>, check_interval: Duration) -> (Self, CompactionWorkerHandle) {
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = CompactionWorkerHandle {
            shutdown: shutdown.clone(),
        };

        let worker = CompactionWorker {
            compactor,
            check_interval,
            shutdown,
        };

        (worker, handle)
    }

    /// Run the worker loop
    pub async fn run(mut self) {
        loop {
            if self.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            // Try to compact
            match self.compactor.compact_if_needed().await {
                Ok(Some(result)) => {
                    // Log compaction result (in production, use proper logging)
                    eprintln!(
                        "Compaction complete: removed {} segments, created {}, reclaimed {} bytes",
                        result.segments_removed.len(),
                        if result.segment_created.is_some() { 1 } else { 0 },
                        result.bytes_reclaimed
                    );
                }
                Ok(None) => {
                    // No compaction needed
                }
                Err(CompactionError::NothingToCompact) => {
                    // Expected when segments are too few
                }
                Err(e) => {
                    eprintln!("Compaction error: {}", e);
                }
            }

            // Sleep with shutdown check - use shorter intervals to be responsive
            let sleep_chunk = std::time::Duration::from_millis(100);
            let mut remaining = self.check_interval;
            while remaining > std::time::Duration::ZERO {
                if self.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let sleep_time = remaining.min(sleep_chunk);
                tokio::time::sleep(sleep_time).await;
                remaining = remaining.saturating_sub(sleep_chunk);
            }
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

    fn make_delta(key: &str, value: &str, ts: u64, replica: u64) -> ReplicationDelta {
        let replica_id = ReplicaId::new(replica);
        let clock = LamportClock {
            time: ts,
            replica_id,
        };
        let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }

    fn make_tombstone(key: &str, ts: u64, replica: u64) -> ReplicationDelta {
        let replica_id = ReplicaId::new(replica);
        let mut clock = LamportClock {
            time: ts,
            replica_id,
        };
        let mut replicated = ReplicatedValue::new(replica_id);
        replicated.delete(&mut clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }

    async fn write_segment(
        store: &InMemoryObjectStore,
        key: &str,
        deltas: &[ReplicationDelta],
    ) -> (u64, u64, u64) {
        let mut writer = SegmentWriter::new(Compression::None);
        let mut min_ts = u64::MAX;
        let mut max_ts = 0u64;

        for delta in deltas {
            writer.write_delta(delta).unwrap();
            min_ts = min_ts.min(delta.value.timestamp.time);
            max_ts = max_ts.max(delta.value.timestamp.time);
        }

        let data = writer.finish().unwrap();
        let size = data.len() as u64;
        store.put(key, &data).await.unwrap();
        (size, min_ts, max_ts)
    }

    #[tokio::test]
    async fn test_compaction_basic() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        // Create initial manifest with segments
        let mut manifest = Manifest::new(1);

        // Write segment 0
        let deltas0 = vec![
            make_delta("key1", "v1", 100, 1),
            make_delta("key2", "v2", 101, 1),
        ];
        let (size0, min0, max0) = write_segment(&store, "test/segments/segment-00000000.seg", &deltas0).await;
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 2,
            size_bytes: size0,
            min_timestamp: min0,
            max_timestamp: max0,
        });

        // Write segment 1
        let deltas1 = vec![
            make_delta("key3", "v3", 200, 1),
            make_delta("key4", "v4", 201, 1),
        ];
        let (size1, min1, max1) = write_segment(&store, "test/segments/segment-00000001.seg", &deltas1).await;
        manifest.add_segment(SegmentInfo {
            id: 1,
            key: "test/segments/segment-00000001.seg".to_string(),
            record_count: 2,
            size_bytes: size1,
            min_timestamp: min1,
            max_timestamp: max1,
        });

        manifest.next_segment_id = 2;
        manifest_manager.save(&manifest).await.unwrap();

        // Create compactor with test config
        let mut compactor = Compactor::new(
            store.clone(),
            "test".to_string(),
            manifest_manager.clone(),
            CompactionConfig::test(),
        );

        // Compact
        let result = compactor.compact().await.unwrap();

        assert_eq!(result.segments_removed.len(), 2);
        assert!(result.segment_created.is_some());
        assert_eq!(result.deltas_before, 4);
        assert_eq!(result.deltas_after, 4);
        assert_eq!(result.tombstones_removed, 0);

        // Verify new segment exists
        let new_segment = result.segment_created.unwrap();
        assert!(store.exists(&new_segment.key).await.unwrap());

        // Verify old segments deleted
        assert!(!store.exists("test/segments/segment-00000000.seg").await.unwrap());
        assert!(!store.exists("test/segments/segment-00000001.seg").await.unwrap());

        // Verify manifest updated
        let new_manifest = manifest_manager.load().await.unwrap();
        assert_eq!(new_manifest.segments.len(), 1);
        assert_eq!(new_manifest.segments[0].id, new_segment.id);
    }

    #[tokio::test]
    async fn test_compaction_deduplication() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        let mut manifest = Manifest::new(1);

        // Write segment 0 with key1
        let deltas0 = vec![make_delta("key1", "old_value", 100, 1)];
        let (size0, min0, max0) = write_segment(&store, "test/segments/segment-00000000.seg", &deltas0).await;
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 1,
            size_bytes: size0,
            min_timestamp: min0,
            max_timestamp: max0,
        });

        // Write segment 1 with same key1 but newer timestamp
        let deltas1 = vec![make_delta("key1", "new_value", 200, 1)];
        let (size1, min1, max1) = write_segment(&store, "test/segments/segment-00000001.seg", &deltas1).await;
        manifest.add_segment(SegmentInfo {
            id: 1,
            key: "test/segments/segment-00000001.seg".to_string(),
            record_count: 1,
            size_bytes: size1,
            min_timestamp: min1,
            max_timestamp: max1,
        });

        manifest.next_segment_id = 2;
        manifest_manager.save(&manifest).await.unwrap();

        let mut compactor = Compactor::new(
            store.clone(),
            "test".to_string(),
            manifest_manager.clone(),
            CompactionConfig::test(),
        );

        let result = compactor.compact().await.unwrap();

        // Should have deduplicated: 2 deltas -> 1 delta
        assert_eq!(result.deltas_before, 2);
        assert_eq!(result.deltas_after, 1);

        // Verify the new segment contains only the newer value
        let new_segment = result.segment_created.unwrap();
        let data = store.get(&new_segment.key).await.unwrap();
        let reader = SegmentReader::open(&data).unwrap();
        let deltas: Vec<_> = reader.deltas().unwrap().collect();

        assert_eq!(deltas.len(), 1);
        let delta = deltas[0].as_ref().unwrap();
        assert_eq!(delta.key, "key1");
        assert_eq!(delta.value.get().unwrap().to_string(), "new_value");
    }

    #[tokio::test]
    async fn test_compaction_tombstone_removal() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        let mut manifest = Manifest::new(1);

        // Write segment with old tombstone (timestamp 0 - very old)
        let deltas0 = vec![
            make_tombstone("deleted_key", 0, 1),
            make_delta("live_key", "value", 100, 1),
        ];
        let (size0, min0, max0) = write_segment(&store, "test/segments/segment-00000000.seg", &deltas0).await;
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 2,
            size_bytes: size0,
            min_timestamp: min0,
            max_timestamp: max0,
        });

        // Need at least 2 segments for test config
        let deltas1 = vec![make_delta("another_key", "v", 200, 1)];
        let (size1, min1, max1) = write_segment(&store, "test/segments/segment-00000001.seg", &deltas1).await;
        manifest.add_segment(SegmentInfo {
            id: 1,
            key: "test/segments/segment-00000001.seg".to_string(),
            record_count: 1,
            size_bytes: size1,
            min_timestamp: min1,
            max_timestamp: max1,
        });

        manifest.next_segment_id = 2;
        manifest_manager.save(&manifest).await.unwrap();

        let mut config = CompactionConfig::test();
        config.tombstone_ttl = Duration::from_millis(1); // Very short TTL

        // Wait for tombstone to expire
        tokio::time::sleep(Duration::from_millis(10)).await;

        let mut compactor = Compactor::new(
            store.clone(),
            "test".to_string(),
            manifest_manager,
            config,
        );

        let result = compactor.compact().await.unwrap();

        // Tombstone should be removed
        assert_eq!(result.tombstones_removed, 1);
        assert_eq!(result.deltas_after, 2); // live_key and another_key
    }

    #[tokio::test]
    async fn test_compaction_nothing_to_compact() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        // Create manifest with only 1 segment (below min_segments_to_compact)
        let mut manifest = Manifest::new(1);
        let deltas = vec![make_delta("key1", "v1", 100, 1)];
        let (size, min_ts, max_ts) = write_segment(&store, "test/segments/segment-00000000.seg", &deltas).await;
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 1,
            size_bytes: size,
            min_timestamp: min_ts,
            max_timestamp: max_ts,
        });
        manifest.next_segment_id = 1;
        manifest_manager.save(&manifest).await.unwrap();

        let mut compactor = Compactor::new(
            store,
            "test".to_string(),
            manifest_manager,
            CompactionConfig::test(),
        );

        let result = compactor.compact().await;
        assert!(matches!(result, Err(CompactionError::NothingToCompact)));
    }

    #[tokio::test]
    async fn test_compaction_needs_compaction() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        let mut config = CompactionConfig::test();
        config.max_segments = 3;

        let compactor = Compactor::new(
            store.clone(),
            "test".to_string(),
            manifest_manager.clone(),
            config,
        );

        // Empty manifest - no compaction needed
        let manifest = Manifest::new(1);
        manifest_manager.save(&manifest).await.unwrap();
        assert!(!compactor.needs_compaction().await.unwrap());

        // Add segments up to threshold
        let mut manifest = Manifest::new(1);
        for i in 0..3 {
            manifest.add_segment(SegmentInfo {
                id: i,
                key: format!("test/segments/segment-{:08}.seg", i),
                record_count: 1,
                size_bytes: 100,
                min_timestamp: i * 100,
                max_timestamp: i * 100,
            });
        }
        manifest.next_segment_id = 3;
        manifest_manager.save(&manifest).await.unwrap();

        assert!(compactor.needs_compaction().await.unwrap());
    }

    #[tokio::test]
    async fn test_compaction_all_tombstones() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        let mut manifest = Manifest::new(1);

        // Write segments with only old tombstones
        let deltas0 = vec![make_tombstone("key1", 0, 1)];
        let (size0, min0, max0) = write_segment(&store, "test/segments/segment-00000000.seg", &deltas0).await;
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 1,
            size_bytes: size0,
            min_timestamp: min0,
            max_timestamp: max0,
        });

        let deltas1 = vec![make_tombstone("key2", 0, 1)];
        let (size1, min1, max1) = write_segment(&store, "test/segments/segment-00000001.seg", &deltas1).await;
        manifest.add_segment(SegmentInfo {
            id: 1,
            key: "test/segments/segment-00000001.seg".to_string(),
            record_count: 1,
            size_bytes: size1,
            min_timestamp: min1,
            max_timestamp: max1,
        });

        manifest.next_segment_id = 2;
        manifest_manager.save(&manifest).await.unwrap();

        let mut config = CompactionConfig::test();
        config.tombstone_ttl = Duration::from_millis(1);

        tokio::time::sleep(Duration::from_millis(10)).await;

        let mut compactor = Compactor::new(
            store.clone(),
            "test".to_string(),
            manifest_manager.clone(),
            config,
        );

        let result = compactor.compact().await.unwrap();

        // All tombstones removed, no new segment created
        assert_eq!(result.tombstones_removed, 2);
        assert!(result.segment_created.is_none());
        assert_eq!(result.segments_removed.len(), 2);

        // Manifest should have no segments
        let new_manifest = manifest_manager.load().await.unwrap();
        assert!(new_manifest.segments.is_empty());
    }

    #[tokio::test]
    async fn test_compaction_stats() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        let mut manifest = Manifest::new(1);

        for i in 0..3 {
            let deltas = vec![make_delta(&format!("key{}", i), "value", i * 100, 1)];
            let key = format!("test/segments/segment-{:08}.seg", i);
            let (size, min_ts, max_ts) = write_segment(&store, &key, &deltas).await;
            manifest.add_segment(SegmentInfo {
                id: i,
                key,
                record_count: 1,
                size_bytes: size,
                min_timestamp: min_ts,
                max_timestamp: max_ts,
            });
        }
        manifest.next_segment_id = 3;
        manifest_manager.save(&manifest).await.unwrap();

        let mut compactor = Compactor::new(
            store,
            "test".to_string(),
            manifest_manager,
            CompactionConfig::test(),
        );

        compactor.compact().await.unwrap();

        let stats = compactor.stats();
        assert_eq!(stats.compactions_performed, 1);
        assert_eq!(stats.segments_removed, 3);
        assert_eq!(stats.segments_created, 1);
    }

    // DST test
    #[tokio::test]
    async fn test_compaction_with_simulated_store() {
        use crate::io::simulation::SimulatedRng;
        use crate::streaming::{SimulatedObjectStore, SimulatedStoreConfig};

        let inner = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(42);
        let store = Arc::new(SimulatedObjectStore::new(
            inner,
            rng,
            SimulatedStoreConfig::no_faults(),
        ));

        let manifest_manager = ManifestManager::new((*store).clone(), "dst");

        let mut manifest = Manifest::new(1);

        // Create segments
        for i in 0..3 {
            let deltas = vec![make_delta(&format!("key{}", i), "v", i * 100, 1)];

            let mut writer = SegmentWriter::new(Compression::None);
            for d in &deltas {
                writer.write_delta(d).unwrap();
            }
            let data = writer.finish().unwrap();
            let size = data.len() as u64;

            let key = format!("dst/segments/segment-{:08}.seg", i);
            store.put(&key, &data).await.unwrap();

            manifest.add_segment(SegmentInfo {
                id: i,
                key,
                record_count: 1,
                size_bytes: size,
                min_timestamp: i * 100,
                max_timestamp: i * 100,
            });
        }
        manifest.next_segment_id = 3;
        manifest_manager.save(&manifest).await.unwrap();

        let mut compactor = Compactor::new(
            store,
            "dst".to_string(),
            manifest_manager.clone(),
            CompactionConfig::test(),
        );

        let result = compactor.compact().await.unwrap();

        assert_eq!(result.segments_removed.len(), 3);
        assert!(result.segment_created.is_some());
        assert_eq!(result.deltas_after, 3);
    }
}
