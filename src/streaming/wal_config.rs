//! WAL Configuration
//!
//! Defines configuration types for the Write-Ahead Log subsystem.
//! Three fsync policies mirror Redis AOF behavior with DST-verifiable guarantees.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Fsync policy for WAL durability guarantees
///
/// Mirrors Redis AOF `appendfsync` options with clearly-defined RPO bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FsyncPolicy {
    /// Group commit: batch entries + fsync before acknowledging any writer.
    /// RPO = 0 (zero data loss). Latency: ~2-10μs/write amortized.
    Always,
    /// Append + acknowledge immediately; fsync on a 1-second timer.
    /// RPO ≤ 1 second. Latency: ~0.1μs/write.
    EverySecond,
    /// Append + acknowledge immediately; OS decides when to flush.
    /// RPO = unbounded. Latency: ~0.1μs/write.
    No,
}

impl Default for FsyncPolicy {
    fn default() -> Self {
        FsyncPolicy::EverySecond
    }
}

/// Configuration for the Write-Ahead Log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    /// Enable WAL persistence
    pub enabled: bool,
    /// Directory for WAL files
    pub wal_dir: PathBuf,
    /// Fsync policy (determines durability vs. latency tradeoff)
    pub fsync_policy: FsyncPolicy,
    /// Maximum WAL file size before rotation (default: 64MB)
    pub max_file_size: usize,
    /// Maximum entries per group commit batch (default: 64)
    pub group_commit_max_entries: usize,
    /// Maximum wait time for group commit batch (default: 200μs)
    #[serde(with = "duration_micros")]
    pub group_commit_max_wait: Duration,
    /// Interval between WAL truncation checks (default: 30s)
    #[serde(with = "duration_millis")]
    pub truncation_check_interval: Duration,
}

impl Default for WalConfig {
    fn default() -> Self {
        WalConfig {
            enabled: false,
            wal_dir: PathBuf::from("/tmp/redis-wal"),
            fsync_policy: FsyncPolicy::EverySecond,
            max_file_size: 64 * 1024 * 1024, // 64MB
            group_commit_max_entries: 64,
            group_commit_max_wait: Duration::from_micros(200),
            truncation_check_interval: Duration::from_secs(30),
        }
    }
}

impl WalConfig {
    /// Configuration for testing (smaller files, faster intervals)
    pub fn test() -> Self {
        WalConfig {
            enabled: true,
            wal_dir: PathBuf::from("/tmp/redis-wal-test"),
            fsync_policy: FsyncPolicy::Always,
            max_file_size: 64 * 1024, // 64KB for fast rotation in tests
            group_commit_max_entries: 8,
            group_commit_max_wait: Duration::from_micros(50),
            truncation_check_interval: Duration::from_millis(100),
        }
    }

    /// Always-fsync configuration for zero-RPO workloads
    pub fn always_fsync(wal_dir: PathBuf) -> Self {
        WalConfig {
            enabled: true,
            wal_dir,
            fsync_policy: FsyncPolicy::Always,
            ..Default::default()
        }
    }

    /// Every-second configuration for balanced durability/throughput
    pub fn every_second(wal_dir: PathBuf) -> Self {
        WalConfig {
            enabled: true,
            wal_dir,
            fsync_policy: FsyncPolicy::EverySecond,
            ..Default::default()
        }
    }
}

/// Serde helper for Duration as microseconds
mod duration_micros {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_micros().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let micros = u64::deserialize(deserializer)?;
        Ok(Duration::from_micros(micros))
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
        let config = WalConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.fsync_policy, FsyncPolicy::EverySecond);
        assert_eq!(config.max_file_size, 64 * 1024 * 1024);
        assert_eq!(config.group_commit_max_entries, 64);
    }

    #[test]
    fn test_test_config() {
        let config = WalConfig::test();
        assert!(config.enabled);
        assert_eq!(config.fsync_policy, FsyncPolicy::Always);
        assert_eq!(config.max_file_size, 64 * 1024);
    }

    #[test]
    fn test_always_fsync_config() {
        let config = WalConfig::always_fsync(PathBuf::from("/data/wal"));
        assert!(config.enabled);
        assert_eq!(config.fsync_policy, FsyncPolicy::Always);
        assert_eq!(config.wal_dir, PathBuf::from("/data/wal"));
    }

    #[test]
    fn test_config_serialization() {
        let config = WalConfig::test();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: WalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.enabled, parsed.enabled);
        assert_eq!(config.fsync_policy, parsed.fsync_policy);
        assert_eq!(config.max_file_size, parsed.max_file_size);
        assert_eq!(config.group_commit_max_entries, parsed.group_commit_max_entries);
        assert_eq!(config.group_commit_max_wait, parsed.group_commit_max_wait);
    }

    #[test]
    fn test_fsync_policy_default() {
        assert_eq!(FsyncPolicy::default(), FsyncPolicy::EverySecond);
    }
}
