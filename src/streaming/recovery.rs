//! Recovery Manager for Streaming Persistence
//!
//! Restores state from object store on startup. Recovery is idempotent
//! due to CRDT semantics - replaying the same delta multiple times
//! produces the same result.
//!
//! ## Recovery Flow (TigerStyle: explicit steps)
//!
//! 1. Load manifest (or start empty if not found)
//! 2. If checkpoint exists, load it first
//! 3. Load segments after checkpoint timestamp
//! 4. Sort segments by min_timestamp
//! 5. Replay deltas in order
//!
//! ## DST Compatibility
//!
//! All I/O through ObjectStore trait. Recovery is deterministic given
//! the same object store state.

use crate::replication::state::{ReplicatedValue, ReplicationDelta};
use crate::streaming::{
    CheckpointError, CheckpointReader, Manifest, ManifestError, ManifestManager, ObjectStore,
    SegmentError, SegmentInfo, SegmentReader,
};
use std::collections::HashMap;
use std::io::Error as IoError;

/// Error type for recovery operations
#[derive(Debug)]
pub enum RecoveryError {
    /// Manifest error
    Manifest(ManifestError),
    /// Segment error
    Segment(SegmentError),
    /// I/O error
    Io(IoError),
    /// Checkpoint error
    Checkpoint(CheckpointError),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoveryError::Manifest(e) => write!(f, "Manifest error: {}", e),
            RecoveryError::Segment(e) => write!(f, "Segment error: {}", e),
            RecoveryError::Io(e) => write!(f, "I/O error: {}", e),
            RecoveryError::Checkpoint(e) => write!(f, "Checkpoint error: {}", e),
        }
    }
}

impl std::error::Error for RecoveryError {}

impl From<ManifestError> for RecoveryError {
    fn from(e: ManifestError) -> Self {
        RecoveryError::Manifest(e)
    }
}

impl From<SegmentError> for RecoveryError {
    fn from(e: SegmentError) -> Self {
        RecoveryError::Segment(e)
    }
}

impl From<IoError> for RecoveryError {
    fn from(e: IoError) -> Self {
        RecoveryError::Io(e)
    }
}

impl From<CheckpointError> for RecoveryError {
    fn from(e: CheckpointError) -> Self {
        RecoveryError::Checkpoint(e)
    }
}

/// Current phase of recovery
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPhase {
    /// Not started
    NotStarted,
    /// Loading manifest
    LoadingManifest,
    /// Loading checkpoint
    LoadingCheckpoint,
    /// Loading segments
    LoadingSegments,
    /// Replaying deltas
    ReplayingDeltas,
    /// Recovery complete
    Complete,
}

/// Progress information during recovery
#[derive(Debug, Clone)]
pub struct RecoveryProgress {
    /// Current phase
    pub phase: RecoveryPhase,
    /// Total segments to load
    pub segments_total: usize,
    /// Segments loaded so far
    pub segments_loaded: usize,
    /// Total deltas replayed
    pub deltas_replayed: u64,
    /// Bytes read from object store
    pub bytes_read: u64,
}

impl RecoveryProgress {
    fn new() -> Self {
        RecoveryProgress {
            phase: RecoveryPhase::NotStarted,
            segments_total: 0,
            segments_loaded: 0,
            deltas_replayed: 0,
            bytes_read: 0,
        }
    }
}

/// Result of recovery operation
#[derive(Debug)]
pub struct RecoveredState {
    /// Manifest that was loaded
    pub manifest: Manifest,
    /// Checkpoint state (if checkpoint was loaded)
    pub checkpoint_state: Option<HashMap<String, ReplicatedValue>>,
    /// Deltas to replay after checkpoint (in order)
    pub deltas: Vec<ReplicationDelta>,
    /// Recovery statistics
    pub stats: RecoveryStats,
}

/// Statistics from recovery
#[derive(Debug, Clone, Default)]
pub struct RecoveryStats {
    /// Number of segments loaded
    pub segments_loaded: usize,
    /// Number of deltas replayed
    pub deltas_replayed: u64,
    /// Total bytes read
    pub bytes_read: u64,
    /// Whether a checkpoint was used
    pub used_checkpoint: bool,
    /// Segments skipped due to checkpoint
    pub segments_skipped: usize,
}

/// Recovery manager handles loading state from object store
pub struct RecoveryManager<S: ObjectStore + Clone> {
    store: S,
    manifest_manager: ManifestManager<S>,
    replica_id: u64,
}

impl<S: ObjectStore + Clone + 'static> RecoveryManager<S> {
    /// Create a new recovery manager
    pub fn new(store: S, prefix: &str, replica_id: u64) -> Self {
        let manifest_manager = ManifestManager::new(store.clone(), prefix);
        RecoveryManager {
            store,
            manifest_manager,
            replica_id,
        }
    }

    /// Perform full recovery
    ///
    /// Returns checkpoint state (if available) and deltas to replay.
    /// Deltas are returned in timestamp order for deterministic replay.
    pub async fn recover(&self) -> Result<RecoveredState, RecoveryError> {
        let mut stats = RecoveryStats::default();

        // Step 1: Load manifest
        let manifest = self
            .manifest_manager
            .load_or_create(self.replica_id)
            .await?;

        // Step 2: Load checkpoint if available
        let (checkpoint_state, last_checkpoint_segment) =
            if let Some(ref checkpoint_info) = manifest.checkpoint {
                stats.used_checkpoint = true;

                // Load checkpoint data from object store
                let checkpoint_data = self.store.get(&checkpoint_info.key).await?;
                stats.bytes_read += checkpoint_data.len() as u64;

                let reader = CheckpointReader::open(&checkpoint_data)?;
                reader.validate()?;

                let data = reader.load()?;
                (Some(data.state), checkpoint_info.last_segment_id)
            } else {
                (None, 0)
            };

        // Step 3: Filter segments after checkpoint and sort
        let mut segments_to_load: Vec<&SegmentInfo> = if checkpoint_state.is_some() {
            // Only load segments after the checkpoint
            manifest
                .segments
                .iter()
                .filter(|s| s.id > last_checkpoint_segment)
                .collect()
        } else {
            // Load all segments
            manifest.segments.iter().collect()
        };

        // Sort by min_timestamp for deterministic ordering
        segments_to_load.sort_by_key(|s| s.min_timestamp);

        stats.segments_skipped = manifest.segments.len() - segments_to_load.len();

        // Step 4: Load segments and collect deltas
        let mut all_deltas = Vec::new();

        for segment_info in segments_to_load {
            let segment_deltas = self.load_segment(segment_info).await?;
            stats.bytes_read += segment_info.size_bytes;
            stats.segments_loaded += 1;
            stats.deltas_replayed += segment_deltas.len() as u64;
            all_deltas.extend(segment_deltas);
        }

        Ok(RecoveredState {
            manifest,
            checkpoint_state,
            deltas: all_deltas,
            stats,
        })
    }

    /// Recover with progress callback
    ///
    /// Allows monitoring recovery progress for UI feedback.
    pub async fn recover_with_progress<F>(
        &self,
        mut on_progress: F,
    ) -> Result<RecoveredState, RecoveryError>
    where
        F: FnMut(&RecoveryProgress),
    {
        let mut progress = RecoveryProgress::new();
        let mut stats = RecoveryStats::default();

        // Step 1: Load manifest
        progress.phase = RecoveryPhase::LoadingManifest;
        on_progress(&progress);

        let manifest = self
            .manifest_manager
            .load_or_create(self.replica_id)
            .await?;

        // Step 2: Load checkpoint if available
        let (checkpoint_state, last_checkpoint_segment) =
            if let Some(ref checkpoint_info) = manifest.checkpoint {
                progress.phase = RecoveryPhase::LoadingCheckpoint;
                on_progress(&progress);
                stats.used_checkpoint = true;

                // Load checkpoint data from object store
                let checkpoint_data = self.store.get(&checkpoint_info.key).await?;
                stats.bytes_read += checkpoint_data.len() as u64;

                let reader = CheckpointReader::open(&checkpoint_data)?;
                reader.validate()?;

                let data = reader.load()?;
                (Some(data.state), checkpoint_info.last_segment_id)
            } else {
                (None, 0)
            };

        // Step 3: Filter segments after checkpoint and sort
        let mut segments_to_load: Vec<&SegmentInfo> = if checkpoint_state.is_some() {
            manifest
                .segments
                .iter()
                .filter(|s| s.id > last_checkpoint_segment)
                .collect()
        } else {
            manifest.segments.iter().collect()
        };

        segments_to_load.sort_by_key(|s| s.min_timestamp);
        stats.segments_skipped = manifest.segments.len() - segments_to_load.len();

        progress.phase = RecoveryPhase::LoadingSegments;
        progress.segments_total = segments_to_load.len();
        on_progress(&progress);

        // Step 4: Load segments
        let mut all_deltas = Vec::new();

        for segment_info in segments_to_load {
            let segment_deltas = self.load_segment(segment_info).await?;

            stats.bytes_read += segment_info.size_bytes;
            stats.segments_loaded += 1;
            stats.deltas_replayed += segment_deltas.len() as u64;

            progress.segments_loaded = stats.segments_loaded;
            progress.bytes_read = stats.bytes_read;
            progress.deltas_replayed = stats.deltas_replayed;
            on_progress(&progress);

            all_deltas.extend(segment_deltas);
        }

        progress.phase = RecoveryPhase::Complete;
        on_progress(&progress);

        Ok(RecoveredState {
            manifest,
            checkpoint_state,
            deltas: all_deltas,
            stats,
        })
    }

    /// Load a single segment and extract deltas
    async fn load_segment(
        &self,
        segment_info: &SegmentInfo,
    ) -> Result<Vec<ReplicationDelta>, RecoveryError> {
        let data = self.store.get(&segment_info.key).await?;
        let reader = SegmentReader::open(&data)?;

        // Validate segment integrity
        reader.validate()?;

        // Extract all deltas
        let deltas: Result<Vec<_>, _> = reader.deltas()?.collect();
        Ok(deltas?)
    }

    /// Check if recovery is needed
    pub async fn needs_recovery(&self) -> Result<bool, RecoveryError> {
        Ok(self.manifest_manager.exists().await?)
    }

    /// Get manifest manager for external use
    pub fn manifest_manager(&self) -> &ManifestManager<S> {
        &self.manifest_manager
    }

    /// Perform recovery with WAL replay phase.
    ///
    /// 1. Load from object store (checkpoint + segments) — bulk state
    /// 2. Determine high-water mark (max timestamp from segments)
    /// 3. Replay WAL entries with timestamp > high_water_mark
    /// 4. CRDT idempotency makes duplicate replay safe — no dedup needed
    pub async fn recover_with_wal<W: crate::streaming::wal_store::WalStore>(
        &self,
        wal_rotator: &crate::streaming::wal::WalRotator<W>,
    ) -> Result<RecoveredState, RecoveryError> {
        let mut recovered = self.recover().await?;

        // Determine high-water mark from object store segments
        let high_water = recovered
            .manifest
            .segments
            .iter()
            .map(|s| s.max_timestamp)
            .max()
            .unwrap_or(0);

        // Replay WAL entries after the high-water mark
        let wal_deltas = wal_rotator
            .recover_entries_after(high_water)
            .map_err(|e| RecoveryError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("WAL recovery failed: {}", e),
            )))?;

        if !wal_deltas.is_empty() {
            recovered.stats.deltas_replayed = recovered
                .stats
                .deltas_replayed
                .checked_add(wal_deltas.len() as u64)
                .unwrap_or(recovered.stats.deltas_replayed);
            recovered.deltas.extend(wal_deltas);
        }

        Ok(recovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;
    use crate::replication::lattice::{LamportClock, ReplicaId};
    use crate::replication::state::ReplicatedValue;
    use crate::streaming::{
        Compression, InMemoryObjectStore, Manifest, SegmentInfo, SegmentWriter,
    };

    fn make_delta(key: &str, value: &str, ts: u64) -> ReplicationDelta {
        let replica_id = ReplicaId::new(1);
        let clock = LamportClock {
            time: ts,
            replica_id,
        };
        let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }

    async fn write_segment(
        store: &InMemoryObjectStore,
        key: &str,
        deltas: &[ReplicationDelta],
    ) -> u64 {
        let mut writer = SegmentWriter::new(Compression::None);
        for delta in deltas {
            writer.write_delta(delta).unwrap();
        }
        let data = writer.finish().unwrap();
        let size = data.len() as u64;
        store.put(key, &data).await.unwrap();
        size
    }

    #[tokio::test]
    async fn test_recovery_empty() {
        let store = InMemoryObjectStore::new();
        let recovery = RecoveryManager::new(store, "test", 1);

        let result = recovery.recover().await.unwrap();

        assert_eq!(result.manifest.replica_id, 1);
        assert!(result.deltas.is_empty());
        assert_eq!(result.stats.segments_loaded, 0);
    }

    #[tokio::test]
    async fn test_recovery_single_segment() {
        let store = InMemoryObjectStore::new();

        // Write a segment
        let deltas = vec![
            make_delta("key1", "value1", 100),
            make_delta("key2", "value2", 200),
        ];
        let size = write_segment(&store, "test/segments/segment-00000000.seg", &deltas).await;

        // Create and save manifest
        let manifest_manager = ManifestManager::new(store.clone(), "test");
        let mut manifest = Manifest::new(1);
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 2,
            size_bytes: size,
            min_timestamp: 100,
            max_timestamp: 200,
        });
        manifest_manager.save(&manifest).await.unwrap();

        // Recover
        let recovery = RecoveryManager::new(store, "test", 1);
        let result = recovery.recover().await.unwrap();

        assert_eq!(result.deltas.len(), 2);
        assert_eq!(result.deltas[0].key, "key1");
        assert_eq!(result.deltas[1].key, "key2");
        assert_eq!(result.stats.segments_loaded, 1);
        assert_eq!(result.stats.deltas_replayed, 2);
    }

    #[tokio::test]
    async fn test_recovery_multiple_segments_ordered() {
        let store = InMemoryObjectStore::new();

        // Write segments with different timestamps
        let deltas1 = vec![make_delta("key1", "v1", 100)];
        let size1 = write_segment(&store, "test/segments/segment-00000000.seg", &deltas1).await;

        let deltas2 = vec![make_delta("key2", "v2", 300)];
        let size2 = write_segment(&store, "test/segments/segment-00000001.seg", &deltas2).await;

        let deltas3 = vec![make_delta("key3", "v3", 200)];
        let size3 = write_segment(&store, "test/segments/segment-00000002.seg", &deltas3).await;

        // Create manifest (segments not in timestamp order)
        let manifest_manager = ManifestManager::new(store.clone(), "test");
        let mut manifest = Manifest::new(1);
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 1,
            size_bytes: size1,
            min_timestamp: 100,
            max_timestamp: 100,
        });
        manifest.add_segment(SegmentInfo {
            id: 1,
            key: "test/segments/segment-00000001.seg".to_string(),
            record_count: 1,
            size_bytes: size2,
            min_timestamp: 300,
            max_timestamp: 300,
        });
        manifest.add_segment(SegmentInfo {
            id: 2,
            key: "test/segments/segment-00000002.seg".to_string(),
            record_count: 1,
            size_bytes: size3,
            min_timestamp: 200,
            max_timestamp: 200,
        });
        manifest_manager.save(&manifest).await.unwrap();

        // Recover - should be sorted by timestamp
        let recovery = RecoveryManager::new(store, "test", 1);
        let result = recovery.recover().await.unwrap();

        assert_eq!(result.deltas.len(), 3);
        // Should be ordered by min_timestamp
        assert_eq!(result.deltas[0].key, "key1"); // ts 100
        assert_eq!(result.deltas[1].key, "key3"); // ts 200
        assert_eq!(result.deltas[2].key, "key2"); // ts 300
    }

    #[tokio::test]
    async fn test_recovery_with_progress() {
        let store = InMemoryObjectStore::new();

        // Write a segment
        let deltas = vec![make_delta("key1", "value1", 100)];
        let size = write_segment(&store, "test/segments/segment-00000000.seg", &deltas).await;

        let manifest_manager = ManifestManager::new(store.clone(), "test");
        let mut manifest = Manifest::new(1);
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "test/segments/segment-00000000.seg".to_string(),
            record_count: 1,
            size_bytes: size,
            min_timestamp: 100,
            max_timestamp: 100,
        });
        manifest_manager.save(&manifest).await.unwrap();

        // Track progress
        let mut phases_seen = Vec::new();
        let recovery = RecoveryManager::new(store, "test", 1);
        let result = recovery
            .recover_with_progress(|progress| {
                phases_seen.push(progress.phase);
            })
            .await
            .unwrap();

        assert!(phases_seen.contains(&RecoveryPhase::LoadingManifest));
        assert!(phases_seen.contains(&RecoveryPhase::LoadingSegments));
        assert!(phases_seen.contains(&RecoveryPhase::Complete));
        assert_eq!(result.deltas.len(), 1);
    }

    #[tokio::test]
    async fn test_recovery_needs_recovery() {
        let store = InMemoryObjectStore::new();
        let recovery = RecoveryManager::new(store.clone(), "test", 1);

        // No manifest - no recovery needed
        assert!(!recovery.needs_recovery().await.unwrap());

        // Save manifest
        let manifest_manager = ManifestManager::new(store, "test");
        manifest_manager.save(&Manifest::new(1)).await.unwrap();

        // Now recovery is needed
        assert!(recovery.needs_recovery().await.unwrap());
    }

    // DST test
    #[tokio::test]
    async fn test_recovery_with_simulated_store() {
        use crate::io::simulation::SimulatedRng;
        use crate::streaming::{SimulatedObjectStore, SimulatedStoreConfig};

        let inner = InMemoryObjectStore::new();

        // Write segment and manifest to inner store first
        let deltas = vec![make_delta("key1", "value1", 100)];
        let size = write_segment(&inner, "dst/segments/segment-00000000.seg", &deltas).await;

        let manifest_manager = ManifestManager::new(inner.clone(), "dst");
        let mut manifest = Manifest::new(1);
        manifest.add_segment(SegmentInfo {
            id: 0,
            key: "dst/segments/segment-00000000.seg".to_string(),
            record_count: 1,
            size_bytes: size,
            min_timestamp: 100,
            max_timestamp: 100,
        });
        manifest_manager.save(&manifest).await.unwrap();

        // Now wrap with simulated store
        let rng = SimulatedRng::new(42);
        let store = SimulatedObjectStore::new(inner, rng, SimulatedStoreConfig::no_faults());

        let recovery = RecoveryManager::new(store, "dst", 1);
        let result = recovery.recover().await.unwrap();

        assert_eq!(result.deltas.len(), 1);
        assert_eq!(result.deltas[0].key, "key1");
    }
}
