//! Configuration for Streaming Persistence
//!
//! Defines configuration structs for the streaming persistence module.

use crate::streaming::wal_config::WalConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Main configuration for streaming persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    /// Enable streaming persistence
    pub enabled: bool,
    /// Object store type
    pub store_type: ObjectStoreType,
    /// Key prefix for all objects in the store
    pub prefix: String,
    /// Local filesystem path (for LocalFs store)
    pub local_path: Option<PathBuf>,
    /// S3 configuration (for S3 store)
    #[cfg(feature = "s3")]
    pub s3: Option<S3Config>,
    /// Write buffer settings
    pub write_buffer: WriteBufferConfig,
    /// Checkpoint settings
    pub checkpoint: CheckpointConfig,
    /// Compaction settings
    pub compaction: CompactionConfig,
    /// WAL settings (optional — disabled by default)
    pub wal: Option<WalConfig>,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        StreamingConfig {
            enabled: false,
            store_type: ObjectStoreType::InMemory,
            prefix: "redis-stream".to_string(),
            local_path: None,
            #[cfg(feature = "s3")]
            s3: None,
            write_buffer: WriteBufferConfig::default(),
            checkpoint: CheckpointConfig::default(),
            compaction: CompactionConfig::default(),
            wal: None,
        }
    }
}

impl StreamingConfig {
    /// Create config for local development
    pub fn local(path: PathBuf) -> Self {
        StreamingConfig {
            enabled: true,
            store_type: ObjectStoreType::LocalFs,
            prefix: "redis-stream".to_string(),
            local_path: Some(path),
            #[cfg(feature = "s3")]
            s3: None,
            write_buffer: WriteBufferConfig::default(),
            checkpoint: CheckpointConfig::default(),
            compaction: CompactionConfig::default(),
            wal: None,
        }
    }

    /// Create config for testing (in-memory)
    pub fn test() -> Self {
        StreamingConfig {
            enabled: true,
            store_type: ObjectStoreType::InMemory,
            prefix: "test".to_string(),
            local_path: None,
            #[cfg(feature = "s3")]
            s3: None,
            write_buffer: WriteBufferConfig::test(),
            checkpoint: CheckpointConfig::test(),
            compaction: CompactionConfig::test(),
            wal: None,
        }
    }
}

/// Type of object store backend
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectStoreType {
    /// In-memory store (for tests)
    InMemory,
    /// Local filesystem
    LocalFs,
    /// Amazon S3 or compatible
    #[cfg(feature = "s3")]
    S3,
}

/// S3 configuration
#[cfg(feature = "s3")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    /// S3 bucket name
    pub bucket: String,
    /// Key prefix within bucket
    pub prefix: String,
    /// AWS region
    pub region: String,
    /// Custom endpoint (for S3-compatible services like MinIO)
    pub endpoint: Option<String>,
}

/// Write buffer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBufferConfig {
    /// Flush interval (default: 250ms)
    #[serde(with = "duration_millis")]
    pub flush_interval: Duration,
    /// Maximum buffer size before forced flush (default: 4MB)
    pub max_size_bytes: usize,
    /// Maximum deltas before forced flush (default: 10,000)
    pub max_deltas: usize,
    /// Backpressure threshold - error if pending exceeds this (default: 16MB)
    pub backpressure_threshold_bytes: usize,
    /// Enable compression for segments
    pub compression_enabled: bool,
}

impl Default for WriteBufferConfig {
    fn default() -> Self {
        WriteBufferConfig {
            flush_interval: Duration::from_millis(250),
            max_size_bytes: 4 * 1024 * 1024, // 4MB
            max_deltas: 10_000,
            backpressure_threshold_bytes: 16 * 1024 * 1024, // 16MB
            compression_enabled: false,
        }
    }
}

impl WriteBufferConfig {
    /// Configuration for tests (smaller buffers, faster flushes)
    pub fn test() -> Self {
        WriteBufferConfig {
            flush_interval: Duration::from_millis(50),
            max_size_bytes: 64 * 1024, // 64KB
            max_deltas: 100,
            backpressure_threshold_bytes: 256 * 1024, // 256KB
            compression_enabled: false,
        }
    }

    /// High-throughput configuration (larger buffers)
    pub fn high_throughput() -> Self {
        WriteBufferConfig {
            flush_interval: Duration::from_millis(500),
            max_size_bytes: 16 * 1024 * 1024, // 16MB
            max_deltas: 50_000,
            backpressure_threshold_bytes: 64 * 1024 * 1024, // 64MB
            compression_enabled: true,
        }
    }
}

/// Checkpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    /// Minimum interval between checkpoints
    #[serde(with = "duration_millis")]
    pub interval: Duration,
    /// Minimum number of segments before checkpointing is worthwhile
    pub min_segments: usize,
    /// Enable compression for checkpoint files
    pub compression_enabled: bool,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        CheckpointConfig {
            interval: Duration::from_secs(3600), // 1 hour
            min_segments: 10,
            compression_enabled: true,
        }
    }
}

impl CheckpointConfig {
    /// Configuration for tests (fast intervals)
    pub fn test() -> Self {
        CheckpointConfig {
            interval: Duration::from_millis(100),
            min_segments: 2,
            compression_enabled: false,
        }
    }
}

/// Compaction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Target size for compacted segments (bytes)
    pub target_segment_size: usize,
    /// Maximum number of segments before compaction triggers
    pub max_segments: usize,
    /// Minimum number of segments to compact together
    pub min_segments_to_compact: usize,
    /// Maximum number of segments to compact in one pass
    pub max_segments_per_compaction: usize,
    /// Time-to-live for tombstones (deleted keys) in milliseconds
    #[serde(with = "duration_millis")]
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
    /// Note: max_segments=0 disables compaction worker for fast shutdown in tests
    pub fn test() -> Self {
        CompactionConfig {
            target_segment_size: 1024, // 1KB
            max_segments: 0,           // Disable compaction worker in tests
            min_segments_to_compact: 2,
            max_segments_per_compaction: 5,
            tombstone_ttl: Duration::from_millis(100),
            compression_enabled: false,
        }
    }
}

/// Serde helper for Duration as milliseconds
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_millis().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = StreamingConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.store_type, ObjectStoreType::InMemory);
    }

    #[test]
    fn test_write_buffer_config_serialization() {
        let config = WriteBufferConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: WriteBufferConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.flush_interval, parsed.flush_interval);
        assert_eq!(config.max_size_bytes, parsed.max_size_bytes);
    }

    #[test]
    fn test_local_config() {
        let config = StreamingConfig::local(PathBuf::from("/tmp/redis-stream"));
        assert!(config.enabled);
        assert_eq!(config.store_type, ObjectStoreType::LocalFs);
        assert_eq!(config.local_path, Some(PathBuf::from("/tmp/redis-stream")));
    }
}
