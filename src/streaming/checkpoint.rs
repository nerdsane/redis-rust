//! Checkpoint Management for Streaming Persistence
//!
//! Creates and manages full state snapshots for faster recovery.
//! Checkpoints contain all current key-value state, allowing old segments
//! to be deleted once a checkpoint covers them.
//!
//! ## Architecture (TigerStyle: explicit flow)
//!
//! ```text
//! ReplicatedState → CheckpointManager::create_checkpoint()
//!                         ↓
//!                   CheckpointWriter → ObjectStore
//!                         ↓
//!                   ManifestManager.compact_segments()
//! ```
//!
//! ## DST Compatibility
//!
//! All I/O through ObjectStore trait. Deterministic checkpoint naming.

use crate::io::{ProductionTimeSource, TimeSource};
use crate::replication::state::ReplicatedValue;
use crate::streaming::{Compression, ManifestManager, ObjectStore, SegmentError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Cursor, Read as IoRead, Write as IoWrite};
use std::sync::Arc;

/// Configuration for checkpoint operations
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    /// Minimum interval between checkpoints
    pub interval: std::time::Duration,
    /// Minimum number of segments before checkpointing is worthwhile
    pub min_segments: usize,
    /// Enable compression for checkpoint files
    pub compression_enabled: bool,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        CheckpointConfig {
            interval: std::time::Duration::from_secs(3600), // 1 hour
            min_segments: 10,
            compression_enabled: true,
        }
    }
}

impl CheckpointConfig {
    /// Configuration for tests (fast intervals)
    pub fn test() -> Self {
        CheckpointConfig {
            interval: std::time::Duration::from_millis(100),
            min_segments: 2,
            compression_enabled: false,
        }
    }
}

/// Error type for checkpoint operations
#[derive(Debug)]
pub enum CheckpointError {
    /// I/O error
    Io(std::io::Error),
    /// Segment error (reused for format issues)
    Segment(SegmentError),
    /// Serialization error
    Serialization(String),
    /// Checksum mismatch
    ChecksumMismatch { expected: u32, actual: u32 },
    /// Invalid checkpoint format
    InvalidFormat(String),
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointError::Io(e) => write!(f, "I/O error: {}", e),
            CheckpointError::Segment(e) => write!(f, "Segment error: {}", e),
            CheckpointError::Serialization(msg) => write!(f, "Serialization error: {}", msg),
            CheckpointError::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "Checksum mismatch: expected {:08x}, got {:08x}",
                    expected, actual
                )
            }
            CheckpointError::InvalidFormat(msg) => write!(f, "Invalid format: {}", msg),
        }
    }
}

impl std::error::Error for CheckpointError {}

impl From<std::io::Error> for CheckpointError {
    fn from(e: std::io::Error) -> Self {
        CheckpointError::Io(e)
    }
}

impl From<SegmentError> for CheckpointError {
    fn from(e: SegmentError) -> Self {
        CheckpointError::Segment(e)
    }
}

/// Checkpoint file format
///
/// Similar to segments but contains full state snapshot:
/// - Header: magic, version, flags, key_count
/// - Data: serialized HashMap<String, ReplicatedValue>
/// - Footer: data_checksum, header fields repeated
const CHECKPOINT_MAGIC: &[u8; 4] = b"RCHK";
const CHECKPOINT_VERSION: u8 = 1;

/// Checkpoint header size in bytes
const CHECKPOINT_HEADER_SIZE: usize = 48;

/// Checkpoint header (48 bytes, manually serialized)
#[derive(Debug, Clone)]
struct CheckpointHeader {
    /// Magic bytes "RCHK"
    magic: [u8; 4],
    /// Format version
    version: u8,
    /// Flags (bit 0: compressed)
    flags: u8,
    /// Number of keys in checkpoint
    key_count: u64,
    /// Timestamp when checkpoint was created (epoch ms)
    timestamp_ms: u64,
    /// Last segment ID covered by this checkpoint
    last_segment_id: u64,
    /// Header checksum
    header_checksum: u32,
}

impl CheckpointHeader {
    fn new(key_count: u64, timestamp_ms: u64, last_segment_id: u64, compressed: bool) -> Self {
        let mut header = CheckpointHeader {
            magic: *CHECKPOINT_MAGIC,
            version: CHECKPOINT_VERSION,
            flags: if compressed { 1 } else { 0 },
            key_count,
            timestamp_ms,
            last_segment_id,
            header_checksum: 0,
        };
        header.header_checksum = header.compute_checksum();
        header
    }

    fn compute_checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&self.magic);
        hasher.update(&[self.version, self.flags]);
        hasher.update(&self.key_count.to_le_bytes());
        hasher.update(&self.timestamp_ms.to_le_bytes());
        hasher.update(&self.last_segment_id.to_le_bytes());
        hasher.finalize()
    }

    fn write_to<W: IoWrite>(&self, writer: &mut W) -> Result<(), CheckpointError> {
        // 4 bytes: magic
        writer.write_all(&self.magic)?;
        // 1 byte: version
        writer.write_all(&[self.version])?;
        // 1 byte: flags
        writer.write_all(&[self.flags])?;
        // 2 bytes: padding
        writer.write_all(&[0u8; 2])?;
        // 8 bytes: key_count
        writer.write_all(&self.key_count.to_le_bytes())?;
        // 8 bytes: timestamp_ms
        writer.write_all(&self.timestamp_ms.to_le_bytes())?;
        // 8 bytes: last_segment_id
        writer.write_all(&self.last_segment_id.to_le_bytes())?;
        // 12 bytes: reserved
        writer.write_all(&[0u8; 12])?;
        // 4 bytes: header_checksum
        writer.write_all(&self.header_checksum.to_le_bytes())?;
        Ok(())
    }

    /// Read header from a reader
    ///
    /// # Safety Invariant
    /// All try_into() calls are safe because buf is exactly CHECKPOINT_HEADER_SIZE (48) bytes,
    /// and all slice indices are within bounds.
    fn read_from<R: IoRead>(reader: &mut R) -> Result<Self, CheckpointError> {
        let mut buf = [0u8; CHECKPOINT_HEADER_SIZE];
        reader.read_exact(&mut buf)?;

        // TigerStyle: All try_into() are safe - buf is fixed 48-byte array
        let magic: [u8; 4] = buf[0..4]
            .try_into()
            .expect("buf is 48 bytes, indices 0..4 valid");
        let version = buf[4];
        let flags = buf[5];
        // buf[6..8] is padding
        let key_count = u64::from_le_bytes(
            buf[8..16]
                .try_into()
                .expect("buf is 48 bytes, indices 8..16 valid"),
        );
        let timestamp_ms = u64::from_le_bytes(
            buf[16..24]
                .try_into()
                .expect("buf is 48 bytes, indices 16..24 valid"),
        );
        let last_segment_id = u64::from_le_bytes(
            buf[24..32]
                .try_into()
                .expect("buf is 48 bytes, indices 24..32 valid"),
        );
        // buf[32..44] is reserved
        let header_checksum = u32::from_le_bytes(
            buf[44..48]
                .try_into()
                .expect("buf is 48 bytes, indices 44..48 valid"),
        );

        Ok(CheckpointHeader {
            magic,
            version,
            flags,
            key_count,
            timestamp_ms,
            last_segment_id,
            header_checksum,
        })
    }

    fn validate(&self) -> Result<(), CheckpointError> {
        if &self.magic != CHECKPOINT_MAGIC {
            return Err(CheckpointError::InvalidFormat(format!(
                "Invalid magic: expected RCHK, got {:?}",
                self.magic
            )));
        }
        if self.version != CHECKPOINT_VERSION {
            return Err(CheckpointError::InvalidFormat(format!(
                "Unsupported version: {}",
                self.version
            )));
        }
        let expected = self.compute_checksum();
        if self.header_checksum != expected {
            return Err(CheckpointError::ChecksumMismatch {
                expected,
                actual: self.header_checksum,
            });
        }
        Ok(())
    }

    fn is_compressed(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// Checkpoint footer (16 bytes)
#[derive(Debug, Clone)]
struct CheckpointFooter {
    /// CRC32 of data section
    data_checksum: u32,
    /// Data size (uncompressed)
    data_size: u64,
    /// Footer checksum
    footer_checksum: u32,
}

impl CheckpointFooter {
    fn new(data_checksum: u32, data_size: u64) -> Self {
        let mut footer = CheckpointFooter {
            data_checksum,
            data_size,
            footer_checksum: 0,
        };
        footer.footer_checksum = footer.compute_checksum();
        footer
    }

    fn compute_checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&self.data_checksum.to_le_bytes());
        hasher.update(&self.data_size.to_le_bytes());
        hasher.finalize()
    }

    fn write_to<W: IoWrite>(&self, writer: &mut W) -> Result<(), CheckpointError> {
        writer.write_all(&self.data_checksum.to_le_bytes())?;
        writer.write_all(&self.data_size.to_le_bytes())?;
        writer.write_all(&self.footer_checksum.to_le_bytes())?;
        Ok(())
    }

    fn read_from<R: IoRead>(reader: &mut R) -> Result<Self, CheckpointError> {
        let mut buf4 = [0u8; 4];
        let mut buf8 = [0u8; 8];

        reader.read_exact(&mut buf4)?;
        let data_checksum = u32::from_le_bytes(buf4);

        reader.read_exact(&mut buf8)?;
        let data_size = u64::from_le_bytes(buf8);

        reader.read_exact(&mut buf4)?;
        let footer_checksum = u32::from_le_bytes(buf4);

        let footer = CheckpointFooter {
            data_checksum,
            data_size,
            footer_checksum,
        };

        let expected = footer.compute_checksum();
        if footer_checksum != expected {
            return Err(CheckpointError::ChecksumMismatch {
                expected,
                actual: footer_checksum,
            });
        }

        Ok(footer)
    }
}

/// Checkpoint data format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointData {
    /// All key-value state
    pub state: HashMap<String, ReplicatedValue>,
}

/// Writes checkpoint files
pub struct CheckpointWriter {
    compression: Compression,
}

impl CheckpointWriter {
    /// Create a new checkpoint writer
    pub fn new(compression: Compression) -> Self {
        CheckpointWriter { compression }
    }

    /// Write a checkpoint from state snapshot
    pub fn write(
        &self,
        state: HashMap<String, ReplicatedValue>,
        timestamp_ms: u64,
        last_segment_id: u64,
    ) -> Result<Vec<u8>, CheckpointError> {
        let key_count = state.len() as u64;
        let data = CheckpointData { state };

        // Serialize data
        let serialized =
            bincode::serialize(&data).map_err(|e| CheckpointError::Serialization(e.to_string()))?;

        // Optionally compress
        let (final_data, compressed) = {
            #[cfg(feature = "compression")]
            {
                match self.compression {
                    Compression::None => (serialized.clone(), false),
                    Compression::Zstd { level } => {
                        let compressed = zstd::encode_all(Cursor::new(&serialized), level)
                            .map_err(|e| CheckpointError::Io(e))?;
                        (compressed, true)
                    }
                }
            }
            #[cfg(not(feature = "compression"))]
            {
                let _ = &self.compression; // silence unused warning
                (serialized.clone(), false)
            }
        };

        // Compute data checksum (of uncompressed data for verification)
        let data_checksum = crc32fast::hash(&serialized);

        // Build header
        let header = CheckpointHeader::new(key_count, timestamp_ms, last_segment_id, compressed);

        // Build footer
        let footer = CheckpointFooter::new(data_checksum, serialized.len() as u64);

        // Assemble checkpoint
        let mut output = Vec::new();

        // Write header (48 bytes)
        header.write_to(&mut output)?;

        // Write data length + data
        let data_len = final_data.len() as u32;
        output.extend_from_slice(&data_len.to_le_bytes());
        output.extend_from_slice(&final_data);

        // Write footer
        footer.write_to(&mut output)?;

        Ok(output)
    }
}

/// Reads checkpoint files
pub struct CheckpointReader<'a> {
    data: &'a [u8],
    header: CheckpointHeader,
}

impl<'a> CheckpointReader<'a> {
    /// Open a checkpoint for reading
    pub fn open(data: &'a [u8]) -> Result<Self, CheckpointError> {
        if data.len() < CHECKPOINT_HEADER_SIZE {
            return Err(CheckpointError::InvalidFormat(
                "Checkpoint too small".to_string(),
            ));
        }

        // Read header (48 bytes)
        let mut cursor = Cursor::new(data);
        let header = CheckpointHeader::read_from(&mut cursor)?;

        header.validate()?;

        Ok(CheckpointReader { data, header })
    }

    /// Validate checkpoint integrity
    pub fn validate(&self) -> Result<(), CheckpointError> {
        // Parse data section
        let data_offset = CHECKPOINT_HEADER_SIZE;
        if self.data.len() < data_offset + 4 {
            return Err(CheckpointError::InvalidFormat(
                "Missing data length".to_string(),
            ));
        }

        let data_len = u32::from_le_bytes([
            self.data[data_offset],
            self.data[data_offset + 1],
            self.data[data_offset + 2],
            self.data[data_offset + 3],
        ]) as usize;

        let data_start = data_offset + 4;
        let data_end = data_start + data_len;
        let footer_start = data_end;

        if self.data.len() < footer_start + 16 {
            return Err(CheckpointError::InvalidFormat("Missing footer".to_string()));
        }

        // Read footer
        let mut footer_cursor = Cursor::new(&self.data[footer_start..]);
        let footer = CheckpointFooter::read_from(&mut footer_cursor)?;

        // Decompress if needed and verify checksum
        let compressed_data = &self.data[data_start..data_end];
        let uncompressed = if self.header.is_compressed() {
            #[cfg(feature = "compression")]
            {
                zstd::decode_all(Cursor::new(compressed_data))
                    .map_err(|e| CheckpointError::Io(e))?
            }
            #[cfg(not(feature = "compression"))]
            {
                return Err(CheckpointError::InvalidFormat(
                    "Compression not enabled".to_string(),
                ));
            }
        } else {
            compressed_data.to_vec()
        };

        // Verify data checksum
        let actual_checksum = crc32fast::hash(&uncompressed);
        if actual_checksum != footer.data_checksum {
            return Err(CheckpointError::ChecksumMismatch {
                expected: footer.data_checksum,
                actual: actual_checksum,
            });
        }

        // Verify data size
        if uncompressed.len() as u64 != footer.data_size {
            return Err(CheckpointError::InvalidFormat(format!(
                "Data size mismatch: expected {}, got {}",
                footer.data_size,
                uncompressed.len()
            )));
        }

        Ok(())
    }

    /// Load checkpoint data
    pub fn load(&self) -> Result<CheckpointData, CheckpointError> {
        let data_offset = CHECKPOINT_HEADER_SIZE;
        let data_len = u32::from_le_bytes([
            self.data[data_offset],
            self.data[data_offset + 1],
            self.data[data_offset + 2],
            self.data[data_offset + 3],
        ]) as usize;

        let data_start = data_offset + 4;
        let data_end = data_start + data_len;
        let compressed_data = &self.data[data_start..data_end];

        let uncompressed = if self.header.is_compressed() {
            #[cfg(feature = "compression")]
            {
                zstd::decode_all(Cursor::new(compressed_data))
                    .map_err(|e| CheckpointError::Io(e))?
            }
            #[cfg(not(feature = "compression"))]
            {
                return Err(CheckpointError::InvalidFormat(
                    "Compression not enabled".to_string(),
                ));
            }
        } else {
            compressed_data.to_vec()
        };

        let data: CheckpointData = bincode::deserialize(&uncompressed)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;

        Ok(data)
    }

    /// Get key count
    pub fn key_count(&self) -> u64 {
        self.header.key_count
    }

    /// Get timestamp
    pub fn timestamp_ms(&self) -> u64 {
        self.header.timestamp_ms
    }

    /// Get last segment ID covered
    pub fn last_segment_id(&self) -> u64 {
        self.header.last_segment_id
    }

    /// Check if checkpoint is compressed
    pub fn is_compressed(&self) -> bool {
        self.header.is_compressed()
    }
}

/// Manages checkpoint lifecycle
///
/// Generic over `T: TimeSource` for zero-cost abstraction:
/// - Production: `ProductionTimeSource` (ZST, compiles to syscall)
/// - Simulation: `SimulatedTimeSource` (virtual clock)
pub struct CheckpointManager<S: ObjectStore + Clone, T: TimeSource = ProductionTimeSource> {
    store: Arc<S>,
    prefix: String,
    manifest_manager: ManifestManager<S>,
    config: CheckpointConfig,
    time_source: T,
}

/// Production-specific constructors (use ProductionTimeSource)
impl<S: ObjectStore + Clone> CheckpointManager<S, ProductionTimeSource> {
    /// Create a new checkpoint manager with production time source
    pub fn new(
        store: Arc<S>,
        prefix: String,
        manifest_manager: ManifestManager<S>,
        config: CheckpointConfig,
    ) -> Self {
        Self::with_time_source(
            store,
            prefix,
            manifest_manager,
            config,
            ProductionTimeSource::new(),
        )
    }
}

/// Generic implementation that works with any TimeSource
impl<S: ObjectStore + Clone, T: TimeSource> CheckpointManager<S, T> {
    /// Create a new checkpoint manager with custom time source
    ///
    /// This is the main constructor - all other constructors delegate to this.
    pub fn with_time_source(
        store: Arc<S>,
        prefix: String,
        manifest_manager: ManifestManager<S>,
        config: CheckpointConfig,
        time_source: T,
    ) -> Self {
        CheckpointManager {
            store,
            prefix,
            manifest_manager,
            config,
            time_source,
        }
    }

    /// Create a checkpoint from current state
    ///
    /// The state should be a snapshot of all key-value pairs.
    /// Uses TimeSource for zero-cost abstraction (syscall in production, virtual clock in simulation).
    pub async fn create_checkpoint(
        &self,
        state: HashMap<String, ReplicatedValue>,
        last_segment_id: u64,
    ) -> Result<CheckpointResult, CheckpointError> {
        let timestamp_ms = self.time_source.now_millis();

        let key_count = state.len() as u64;

        // Determine compression
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

        // Write checkpoint
        let writer = CheckpointWriter::new(compression);
        let checkpoint_data = writer.write(state, timestamp_ms, last_segment_id)?;
        let size_bytes = checkpoint_data.len() as u64;

        // Generate checkpoint key
        let checkpoint_key = format!("{}/checkpoints/chk-{:016}.chk", self.prefix, timestamp_ms);

        // Upload to object store
        self.store.put(&checkpoint_key, &checkpoint_data).await?;

        Ok(CheckpointResult {
            key: checkpoint_key,
            timestamp_ms,
            key_count,
            size_bytes,
            last_segment_id,
        })
    }

    /// Check if a checkpoint should be created
    pub async fn should_checkpoint(&self) -> Result<bool, CheckpointError> {
        let manifest = self.manifest_manager.load_or_create(0).await.map_err(|e| {
            CheckpointError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;

        // Check minimum segments threshold
        if manifest.segments.len() < self.config.min_segments {
            return Ok(false);
        }

        // Check if we have a checkpoint and if it's old enough
        // Uses TimeSource for zero-cost abstraction
        if let Some(ref checkpoint) = manifest.checkpoint {
            let now_ms = self.time_source.now_millis();

            let elapsed_ms = now_ms.saturating_sub(checkpoint.timestamp_ms);
            let interval_ms = self.config.interval.as_millis() as u64;

            if elapsed_ms < interval_ms {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Load an existing checkpoint
    pub async fn load_checkpoint(&self, key: &str) -> Result<CheckpointData, CheckpointError> {
        let data = self.store.get(key).await?;
        let reader = CheckpointReader::open(&data)?;
        reader.validate()?;
        reader.load()
    }

    /// Get configuration
    pub fn config(&self) -> &CheckpointConfig {
        &self.config
    }
}

/// Result of creating a checkpoint
#[derive(Debug, Clone)]
pub struct CheckpointResult {
    /// Object store key for the checkpoint
    pub key: String,
    /// Timestamp when created
    pub timestamp_ms: u64,
    /// Number of keys in checkpoint
    pub key_count: u64,
    /// Size in bytes
    pub size_bytes: u64,
    /// Last segment ID covered
    pub last_segment_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;
    use crate::replication::lattice::{LamportClock, ReplicaId};
    use crate::streaming::InMemoryObjectStore;

    fn make_state(count: usize) -> HashMap<String, ReplicatedValue> {
        let replica_id = ReplicaId::new(1);
        let mut state = HashMap::new();

        for i in 0..count {
            let clock = LamportClock {
                time: (i + 1) as u64,
                replica_id,
            };
            let value = ReplicatedValue::with_value(SDS::from_str(&format!("value{}", i)), clock);
            state.insert(format!("key{}", i), value);
        }

        state
    }

    #[test]
    fn test_checkpoint_roundtrip() {
        let state = make_state(10);
        let writer = CheckpointWriter::new(Compression::None);

        let data = writer.write(state.clone(), 1000, 5).unwrap();

        let reader = CheckpointReader::open(&data).unwrap();
        reader.validate().unwrap();

        assert_eq!(reader.key_count(), 10);
        assert_eq!(reader.timestamp_ms(), 1000);
        assert_eq!(reader.last_segment_id(), 5);

        let loaded = reader.load().unwrap();
        assert_eq!(loaded.state.len(), 10);

        for (key, value) in &state {
            let loaded_value = loaded.state.get(key).unwrap();
            assert_eq!(value.get(), loaded_value.get());
        }
    }

    #[test]
    fn test_checkpoint_empty_state() {
        let state = HashMap::new();
        let writer = CheckpointWriter::new(Compression::None);

        let data = writer.write(state, 2000, 0).unwrap();

        let reader = CheckpointReader::open(&data).unwrap();
        reader.validate().unwrap();

        assert_eq!(reader.key_count(), 0);

        let loaded = reader.load().unwrap();
        assert!(loaded.state.is_empty());
    }

    #[test]
    fn test_checkpoint_large_state() {
        let state = make_state(1000);
        let writer = CheckpointWriter::new(Compression::None);

        let data = writer.write(state.clone(), 3000, 10).unwrap();

        let reader = CheckpointReader::open(&data).unwrap();
        reader.validate().unwrap();

        assert_eq!(reader.key_count(), 1000);

        let loaded = reader.load().unwrap();
        assert_eq!(loaded.state.len(), 1000);
    }

    #[test]
    fn test_checkpoint_corruption_detection() {
        let state = make_state(5);
        let writer = CheckpointWriter::new(Compression::None);

        let mut data = writer.write(state, 4000, 3).unwrap();

        // Corrupt data section
        if data.len() > 60 {
            data[55] ^= 0xFF;
        }

        let reader = CheckpointReader::open(&data).unwrap();
        let result = reader.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_checkpoint_header_validation() {
        let state = make_state(3);
        let writer = CheckpointWriter::new(Compression::None);

        let mut data = writer.write(state, 5000, 2).unwrap();

        // Corrupt magic
        data[0] = b'X';

        let result = CheckpointReader::open(&data);
        assert!(matches!(result, Err(CheckpointError::InvalidFormat(_))));
    }

    #[tokio::test]
    async fn test_checkpoint_manager_create() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        // Initialize manifest
        let manifest = crate::streaming::Manifest::new(1);
        manifest_manager.save(&manifest).await.unwrap();

        let manager = CheckpointManager::new(
            store.clone(),
            "test".to_string(),
            manifest_manager,
            CheckpointConfig::test(),
        );

        let state = make_state(5);
        let result = manager.create_checkpoint(state, 3).await.unwrap();

        assert_eq!(result.key_count, 5);
        assert_eq!(result.last_segment_id, 3);
        assert!(result.key.contains("checkpoints/chk-"));

        // Verify we can load it back
        let loaded = manager.load_checkpoint(&result.key).await.unwrap();
        assert_eq!(loaded.state.len(), 5);
    }

    #[tokio::test]
    async fn test_checkpoint_manager_should_checkpoint() {
        let store = Arc::new(InMemoryObjectStore::new());
        let manifest_manager = ManifestManager::new((*store).clone(), "test");

        // Initialize manifest with no segments
        let manifest = crate::streaming::Manifest::new(1);
        manifest_manager.save(&manifest).await.unwrap();

        let manager = CheckpointManager::new(
            store.clone(),
            "test".to_string(),
            manifest_manager.clone(),
            CheckpointConfig::test(),
        );

        // Should not checkpoint with 0 segments
        assert!(!manager.should_checkpoint().await.unwrap());

        // Add segments
        let mut manifest = manifest_manager.load().await.unwrap();
        for i in 0..3 {
            manifest.add_segment(crate::streaming::SegmentInfo {
                id: i,
                key: format!("segments/segment-{:08}.seg", i),
                record_count: 100,
                size_bytes: 1000,
                min_timestamp: i * 100,
                max_timestamp: (i + 1) * 100,
            });
        }
        manifest_manager.save(&manifest).await.unwrap();

        // Should checkpoint now (min_segments = 2 in test config)
        assert!(manager.should_checkpoint().await.unwrap());
    }

    // DST test with simulated store
    #[tokio::test]
    async fn test_checkpoint_with_simulated_store() {
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

        // Initialize manifest
        let manifest = crate::streaming::Manifest::new(1);
        manifest_manager.save(&manifest).await.unwrap();

        let manager = CheckpointManager::new(
            store.clone(),
            "dst".to_string(),
            manifest_manager,
            CheckpointConfig::test(),
        );

        let state = make_state(10);
        let result = manager.create_checkpoint(state, 5).await.unwrap();

        let loaded = manager.load_checkpoint(&result.key).await.unwrap();
        assert_eq!(loaded.state.len(), 10);
    }

    #[cfg(feature = "compression")]
    #[test]
    fn test_checkpoint_compressed() {
        let state = make_state(100);
        let writer = CheckpointWriter::new(Compression::Zstd { level: 3 });

        let data = writer.write(state.clone(), 6000, 10).unwrap();

        let reader = CheckpointReader::open(&data).unwrap();
        assert!(reader.is_compressed());
        reader.validate().unwrap();

        let loaded = reader.load().unwrap();
        assert_eq!(loaded.state.len(), 100);
    }
}
