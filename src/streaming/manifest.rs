//! Manifest Management for Streaming Persistence
//!
//! Tracks active segments with atomic updates. The manifest is the source of truth
//! for which segments exist and should be used during recovery.
//!
//! ## Atomic Update Pattern (TigerStyle)
//!
//! 1. Write manifest to temp file
//! 2. Rename temp to final (atomic on POSIX)
//! 3. On failure: temp file is orphaned, original intact
//!
//! ## DST Compatibility
//!
//! All I/O goes through ObjectStore trait, enabling fault injection.

use crate::streaming::ObjectStore;
use serde::{Deserialize, Serialize};
use std::io::{Error as IoError, ErrorKind};

/// Error type for manifest operations
#[derive(Debug)]
pub enum ManifestError {
    /// I/O error from object store
    Io(IoError),
    /// JSON serialization/deserialization error
    Json(serde_json::Error),
    /// Manifest not found
    NotFound,
    /// Version conflict during update
    VersionConflict { expected: u64, actual: u64 },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "I/O error: {}", e),
            ManifestError::Json(e) => write!(f, "JSON error: {}", e),
            ManifestError::NotFound => write!(f, "Manifest not found"),
            ManifestError::VersionConflict { expected, actual } => {
                write!(f, "Version conflict: expected {}, got {}", expected, actual)
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<IoError> for ManifestError {
    fn from(e: IoError) -> Self {
        if e.kind() == ErrorKind::NotFound {
            ManifestError::NotFound
        } else {
            ManifestError::Io(e)
        }
    }
}

impl From<serde_json::Error> for ManifestError {
    fn from(e: serde_json::Error) -> Self {
        ManifestError::Json(e)
    }
}

/// Information about a segment file
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentInfo {
    /// Unique segment identifier (monotonically increasing)
    pub id: u64,
    /// Object store key for this segment
    pub key: String,
    /// Number of deltas in segment
    pub record_count: u32,
    /// Size in bytes
    pub size_bytes: u64,
    /// Minimum timestamp (Lamport) in segment
    pub min_timestamp: u64,
    /// Maximum timestamp (Lamport) in segment
    pub max_timestamp: u64,
}

/// Information about a checkpoint file
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointInfo {
    /// Object store key for this checkpoint
    pub key: String,
    /// Timestamp when checkpoint was created
    pub timestamp_ms: u64,
    /// Number of keys in checkpoint
    pub key_count: u64,
    /// Segment ID up to which this checkpoint covers
    pub last_segment_id: u64,
}

/// The manifest tracks all segments and checkpoints
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Manifest version (incremented on each update)
    pub version: u64,
    /// Replica ID that owns this manifest
    pub replica_id: u64,
    /// Active segments (sorted by id)
    pub segments: Vec<SegmentInfo>,
    /// Latest checkpoint (if any)
    pub checkpoint: Option<CheckpointInfo>,
    /// Next segment ID to use
    pub next_segment_id: u64,
}

impl Manifest {
    /// Create a new empty manifest
    pub fn new(replica_id: u64) -> Self {
        Manifest {
            version: 0,
            replica_id,
            segments: Vec::new(),
            checkpoint: None,
            next_segment_id: 0,
        }
    }

    /// Add a segment to the manifest
    pub fn add_segment(&mut self, info: SegmentInfo) {
        // Maintain sorted order by id
        let pos = self.segments.partition_point(|s| s.id < info.id);
        self.segments.insert(pos, info);
        self.version += 1;

        #[cfg(debug_assertions)]
        self.verify_invariants();
    }

    /// Remove segments that are covered by a checkpoint
    pub fn compact_segments(&mut self, checkpoint: CheckpointInfo) {
        // Remove segments with id <= checkpoint.last_segment_id
        self.segments.retain(|s| s.id > checkpoint.last_segment_id);
        self.checkpoint = Some(checkpoint);
        self.version += 1;

        #[cfg(debug_assertions)]
        self.verify_invariants();
    }

    /// Get segments after a given timestamp (for recovery)
    pub fn segments_after(&self, timestamp: u64) -> Vec<&SegmentInfo> {
        self.segments
            .iter()
            .filter(|s| s.max_timestamp >= timestamp)
            .collect()
    }

    /// Allocate next segment ID
    pub fn allocate_segment_id(&mut self) -> u64 {
        let id = self.next_segment_id;
        self.next_segment_id += 1;

        #[cfg(debug_assertions)]
        self.verify_invariants();

        id
    }

    /// TigerStyle: Verify all invariants hold
    ///
    /// # Invariants
    /// - Segments are sorted by id
    /// - next_segment_id > all segment ids
    /// - If checkpoint exists, no segments have id <= checkpoint.last_segment_id
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // Segments must be sorted by id
        debug_assert!(
            self.segments.windows(2).all(|w| w[0].id < w[1].id),
            "Invariant violated: segments must be sorted by id"
        );

        // next_segment_id must be greater than all existing segment ids
        if let Some(last) = self.segments.last() {
            debug_assert!(
                self.next_segment_id > last.id,
                "Invariant violated: next_segment_id ({}) must be > max segment id ({})",
                self.next_segment_id,
                last.id
            );
        }

        // If checkpoint exists, no segments should have id <= checkpoint.last_segment_id
        if let Some(cp) = &self.checkpoint {
            debug_assert!(
                self.segments.iter().all(|s| s.id > cp.last_segment_id),
                "Invariant violated: no segments should have id <= checkpoint.last_segment_id"
            );
        }

        // Segment min_timestamp <= max_timestamp
        for seg in &self.segments {
            debug_assert!(
                seg.min_timestamp <= seg.max_timestamp,
                "Invariant violated: segment {} has min_timestamp > max_timestamp",
                seg.id
            );
        }
    }

    /// Total size of all segments in bytes
    pub fn total_size_bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.size_bytes).sum()
    }

    /// Total record count across all segments
    pub fn total_record_count(&self) -> u64 {
        self.segments.iter().map(|s| s.record_count as u64).sum()
    }
}

/// Manifest manager handles persistence and atomic updates
pub struct ManifestManager<S: ObjectStore> {
    store: S,
    manifest_key: String,
    temp_key: String,
}

impl<S: ObjectStore + Clone> Clone for ManifestManager<S> {
    fn clone(&self) -> Self {
        ManifestManager {
            store: self.store.clone(),
            manifest_key: self.manifest_key.clone(),
            temp_key: self.temp_key.clone(),
        }
    }
}

impl<S: ObjectStore> ManifestManager<S> {
    /// Create a new manifest manager
    pub fn new(store: S, prefix: &str) -> Self {
        ManifestManager {
            store,
            manifest_key: format!("{}/manifest.json", prefix),
            temp_key: format!("{}/manifest.json.tmp", prefix),
        }
    }

    /// Load manifest from object store
    ///
    /// Returns NotFound error if manifest doesn't exist
    pub async fn load(&self) -> Result<Manifest, ManifestError> {
        let data = self.store.get(&self.manifest_key).await?;
        let manifest: Manifest = serde_json::from_slice(&data)?;
        Ok(manifest)
    }

    /// Load manifest or create new one if not found
    pub async fn load_or_create(&self, replica_id: u64) -> Result<Manifest, ManifestError> {
        match self.load().await {
            Ok(manifest) => Ok(manifest),
            Err(ManifestError::NotFound) => Ok(Manifest::new(replica_id)),
            Err(e) => Err(e),
        }
    }

    /// Save manifest atomically
    ///
    /// Uses write-to-temp + rename pattern for atomicity.
    /// This ensures the manifest is never partially written.
    pub async fn save(&self, manifest: &Manifest) -> Result<(), ManifestError> {
        let data = serde_json::to_vec_pretty(manifest)?;

        // Write to temp file first
        self.store.put(&self.temp_key, &data).await?;

        // Atomic rename (on POSIX systems)
        self.store
            .rename(&self.temp_key, &self.manifest_key)
            .await?;

        Ok(())
    }

    /// Update manifest with optimistic locking
    ///
    /// Loads current manifest, applies update function, saves.
    /// Returns VersionConflict if manifest was modified concurrently.
    pub async fn update<F>(&self, updater: F) -> Result<Manifest, ManifestError>
    where
        F: FnOnce(&mut Manifest),
    {
        let mut manifest = self.load().await?;
        let expected_version = manifest.version;

        updater(&mut manifest);

        // Re-check version before save (optimistic locking)
        let current = self.load().await?;
        if current.version != expected_version {
            return Err(ManifestError::VersionConflict {
                expected: expected_version,
                actual: current.version,
            });
        }

        self.save(&manifest).await?;
        Ok(manifest)
    }

    /// Add a segment to manifest atomically
    pub async fn add_segment(&self, info: SegmentInfo) -> Result<Manifest, ManifestError> {
        let mut manifest = self.load().await?;
        manifest.add_segment(info);
        self.save(&manifest).await?;
        Ok(manifest)
    }

    /// Check if manifest exists
    pub async fn exists(&self) -> Result<bool, ManifestError> {
        Ok(self.store.exists(&self.manifest_key).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::InMemoryObjectStore;

    fn make_segment(id: u64, records: u32, size: u64, min_ts: u64, max_ts: u64) -> SegmentInfo {
        SegmentInfo {
            id,
            key: format!("segments/segment-{:08}.seg", id),
            record_count: records,
            size_bytes: size,
            min_timestamp: min_ts,
            max_timestamp: max_ts,
        }
    }

    #[test]
    fn test_manifest_new() {
        let manifest = Manifest::new(1);
        assert_eq!(manifest.version, 0);
        assert_eq!(manifest.replica_id, 1);
        assert!(manifest.segments.is_empty());
        assert!(manifest.checkpoint.is_none());
        assert_eq!(manifest.next_segment_id, 0);
    }

    #[test]
    fn test_manifest_add_segment() {
        let mut manifest = Manifest::new(1);

        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));
        assert_eq!(manifest.segments.len(), 1);
        assert_eq!(manifest.version, 1);

        manifest.add_segment(make_segment(1, 200, 2000, 101, 200));
        assert_eq!(manifest.segments.len(), 2);
        assert_eq!(manifest.version, 2);
    }

    #[test]
    fn test_manifest_add_segment_maintains_order() {
        let mut manifest = Manifest::new(1);

        // Add out of order
        manifest.add_segment(make_segment(2, 100, 1000, 200, 300));
        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));
        manifest.add_segment(make_segment(1, 100, 1000, 100, 200));

        // Should be sorted
        assert_eq!(manifest.segments[0].id, 0);
        assert_eq!(manifest.segments[1].id, 1);
        assert_eq!(manifest.segments[2].id, 2);
    }

    #[test]
    fn test_manifest_compact_segments() {
        let mut manifest = Manifest::new(1);
        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));
        manifest.add_segment(make_segment(1, 100, 1000, 100, 200));
        manifest.add_segment(make_segment(2, 100, 1000, 200, 300));

        let checkpoint = CheckpointInfo {
            key: "checkpoints/chk-1.chk".to_string(),
            timestamp_ms: 1000,
            key_count: 500,
            last_segment_id: 1,
        };

        manifest.compact_segments(checkpoint.clone());

        // Only segment 2 should remain
        assert_eq!(manifest.segments.len(), 1);
        assert_eq!(manifest.segments[0].id, 2);
        assert_eq!(manifest.checkpoint, Some(checkpoint));
    }

    #[test]
    fn test_manifest_segments_after() {
        let mut manifest = Manifest::new(1);
        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));
        manifest.add_segment(make_segment(1, 100, 1000, 100, 200));
        manifest.add_segment(make_segment(2, 100, 1000, 200, 300));

        let after_150 = manifest.segments_after(150);
        assert_eq!(after_150.len(), 2);
        assert_eq!(after_150[0].id, 1);
        assert_eq!(after_150[1].id, 2);
    }

    #[test]
    fn test_manifest_allocate_segment_id() {
        let mut manifest = Manifest::new(1);

        assert_eq!(manifest.allocate_segment_id(), 0);
        assert_eq!(manifest.allocate_segment_id(), 1);
        assert_eq!(manifest.allocate_segment_id(), 2);
        assert_eq!(manifest.next_segment_id, 3);
    }

    #[test]
    fn test_manifest_total_stats() {
        let mut manifest = Manifest::new(1);
        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));
        manifest.add_segment(make_segment(1, 200, 2000, 100, 200));
        manifest.add_segment(make_segment(2, 300, 3000, 200, 300));

        assert_eq!(manifest.total_record_count(), 600);
        assert_eq!(manifest.total_size_bytes(), 6000);
    }

    #[test]
    fn test_manifest_serialization() {
        let mut manifest = Manifest::new(1);
        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));
        manifest.checkpoint = Some(CheckpointInfo {
            key: "checkpoints/chk.chk".to_string(),
            timestamp_ms: 1000,
            key_count: 50,
            last_segment_id: 0,
        });

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();

        assert_eq!(manifest, parsed);
    }

    #[tokio::test]
    async fn test_manifest_manager_save_load() {
        let store = InMemoryObjectStore::new();
        let manager = ManifestManager::new(store, "test");

        let mut manifest = Manifest::new(1);
        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));

        manager.save(&manifest).await.unwrap();

        let loaded = manager.load().await.unwrap();
        assert_eq!(manifest, loaded);
    }

    #[tokio::test]
    async fn test_manifest_manager_load_or_create() {
        let store = InMemoryObjectStore::new();
        let manager = ManifestManager::new(store, "test");

        // Should create new manifest
        let manifest = manager.load_or_create(42).await.unwrap();
        assert_eq!(manifest.replica_id, 42);
        assert_eq!(manifest.version, 0);
    }

    #[tokio::test]
    async fn test_manifest_manager_add_segment() {
        let store = InMemoryObjectStore::new();
        let manager = ManifestManager::new(store, "test");

        // Create initial manifest
        let manifest = Manifest::new(1);
        manager.save(&manifest).await.unwrap();

        // Add segment
        let segment = make_segment(0, 100, 1000, 0, 100);
        let updated = manager.add_segment(segment).await.unwrap();

        assert_eq!(updated.segments.len(), 1);
        assert_eq!(updated.version, 1);
    }

    #[tokio::test]
    async fn test_manifest_manager_not_found() {
        let store = InMemoryObjectStore::new();
        let manager = ManifestManager::new(store, "test");

        let result = manager.load().await;
        assert!(matches!(result, Err(ManifestError::NotFound)));
    }

    #[tokio::test]
    async fn test_manifest_manager_exists() {
        let store = InMemoryObjectStore::new();
        let manager = ManifestManager::new(store, "test");

        assert!(!manager.exists().await.unwrap());

        manager.save(&Manifest::new(1)).await.unwrap();

        assert!(manager.exists().await.unwrap());
    }

    // DST test with simulated store
    #[tokio::test]
    async fn test_manifest_manager_with_simulated_store() {
        use crate::io::simulation::SimulatedRng;
        use crate::streaming::{SimulatedObjectStore, SimulatedStoreConfig};

        let inner = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(12345);
        let store = SimulatedObjectStore::new(inner, rng, SimulatedStoreConfig::no_faults());

        let manager = ManifestManager::new(store, "dst-test");

        let mut manifest = Manifest::new(1);
        manifest.add_segment(make_segment(0, 100, 1000, 0, 100));

        manager.save(&manifest).await.unwrap();
        let loaded = manager.load().await.unwrap();

        assert_eq!(manifest, loaded);
    }
}
