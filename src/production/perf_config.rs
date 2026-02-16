//! Performance Configuration - Tunable parameters for evolutionary optimization
//!
//! This module provides a configuration system that allows performance parameters
//! to be tuned without recompiling. Used by the RedisEvolve harness to discover
//! optimal configurations through evolutionary optimization.

use serde::Deserialize;
use std::path::Path;

/// Performance tuning parameters for the Redis server
#[derive(Debug, Clone, Deserialize)]
pub struct PerformanceConfig {
    /// Number of shards for data partitioning (default: 16)
    #[serde(default = "default_num_shards")]
    pub num_shards: usize,

    /// Response pool configuration
    #[serde(default)]
    pub response_pool: ResponsePoolConfig,

    /// Buffer configuration
    #[serde(default)]
    pub buffers: BufferConfig,

    /// Batching configuration
    #[serde(default)]
    pub batching: BatchingConfig,
}

/// Response pool parameters for reducing channel allocation overhead
#[derive(Debug, Clone, Deserialize)]
pub struct ResponsePoolConfig {
    /// Maximum number of response slots in pool (default: 256)
    #[serde(default = "default_pool_capacity")]
    pub capacity: usize,

    /// Number of slots to pre-allocate at startup (default: 64)
    #[serde(default = "default_pool_prewarm")]
    pub prewarm: usize,
}

/// Buffer size parameters for connection handling
#[derive(Debug, Clone, Deserialize)]
pub struct BufferConfig {
    /// Size of read buffer in bytes (default: 8192)
    #[serde(default = "default_read_buffer")]
    pub read_size: usize,

    /// Maximum buffer size per connection in bytes (default: 1MB)
    #[serde(default = "default_max_buffer")]
    pub max_size: usize,
}

/// Batching parameters for pipeline optimization
#[derive(Debug, Clone, Deserialize)]
pub struct BatchingConfig {
    /// Minimum buffer size to attempt batching (default: 60)
    #[serde(default = "default_min_pipeline_buffer")]
    pub min_pipeline_buffer: usize,

    /// Minimum commands to trigger batch execution (default: 2)
    #[serde(default = "default_batch_threshold")]
    pub batch_threshold: usize,
}

// Default value functions for serde
fn default_num_shards() -> usize {
    16
}
fn default_pool_capacity() -> usize {
    256
}
fn default_pool_prewarm() -> usize {
    64
}
fn default_read_buffer() -> usize {
    8192
}
fn default_max_buffer() -> usize {
    512 * 1024 * 1024
} // 512MB (matches Redis proto-max-bulk-len default)
fn default_min_pipeline_buffer() -> usize {
    60
}
fn default_batch_threshold() -> usize {
    2
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            num_shards: default_num_shards(),
            response_pool: ResponsePoolConfig::default(),
            buffers: BufferConfig::default(),
            batching: BatchingConfig::default(),
        }
    }
}

impl Default for ResponsePoolConfig {
    fn default() -> Self {
        Self {
            capacity: default_pool_capacity(),
            prewarm: default_pool_prewarm(),
        }
    }
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            read_size: default_read_buffer(),
            max_size: default_max_buffer(),
        }
    }
}

impl Default for BatchingConfig {
    fn default() -> Self {
        Self {
            min_pipeline_buffer: default_min_pipeline_buffer(),
            batch_threshold: default_batch_threshold(),
        }
    }
}

impl PerformanceConfig {
    /// Load configuration from a TOML file
    ///
    /// Returns default configuration if file doesn't exist or can't be parsed.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref();

        if !path.exists() {
            tracing::info!("No config file at {:?}, using defaults", path);
            return Self::default();
        }

        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => {
                    tracing::info!("Loaded performance config from {:?}", path);
                    config
                }
                Err(e) => {
                    tracing::warn!("Failed to parse config {:?}: {}, using defaults", path, e);
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read config {:?}: {}, using defaults", path, e);
                Self::default()
            }
        }
    }

    /// Load configuration from environment variable PERF_CONFIG_PATH
    /// or fall back to default path "perf_config.toml"
    pub fn from_env() -> Self {
        let path =
            std::env::var("PERF_CONFIG_PATH").unwrap_or_else(|_| "perf_config.toml".to_string());
        Self::from_file(&path)
    }

    /// Validate configuration values are within acceptable bounds
    pub fn validate(&self) -> Result<(), String> {
        // TigerStyle: Assert preconditions
        if self.num_shards == 0 || self.num_shards > 256 {
            return Err(format!("num_shards must be 1-256, got {}", self.num_shards));
        }
        if !self.num_shards.is_power_of_two() {
            return Err(format!(
                "num_shards must be power of 2, got {}",
                self.num_shards
            ));
        }
        if self.response_pool.capacity == 0 {
            return Err("response_pool.capacity must be > 0".to_string());
        }
        if self.response_pool.prewarm > self.response_pool.capacity {
            return Err("response_pool.prewarm cannot exceed capacity".to_string());
        }
        if self.buffers.read_size == 0 {
            return Err("buffers.read_size must be > 0".to_string());
        }
        if self.buffers.max_size < self.buffers.read_size {
            return Err("buffers.max_size must be >= read_size".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PerformanceConfig::default();
        assert_eq!(config.num_shards, 16);
        assert_eq!(config.response_pool.capacity, 256);
        assert_eq!(config.response_pool.prewarm, 64);
        assert_eq!(config.buffers.read_size, 8192);
        assert_eq!(config.batching.min_pipeline_buffer, 60);
    }

    #[test]
    fn test_validate_valid_config() {
        let config = PerformanceConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_shards() {
        let mut config = PerformanceConfig::default();
        config.num_shards = 0;
        assert!(config.validate().is_err());

        config.num_shards = 300;
        assert!(config.validate().is_err());

        config.num_shards = 15; // Not power of 2
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
            num_shards = 32

            [response_pool]
            capacity = 512
            prewarm = 128

            [buffers]
            read_size = 16384
            max_size = 2097152

            [batching]
            min_pipeline_buffer = 100
            batch_threshold = 4
        "#;

        let config: PerformanceConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.num_shards, 32);
        assert_eq!(config.response_pool.capacity, 512);
        assert_eq!(config.response_pool.prewarm, 128);
        assert_eq!(config.buffers.read_size, 16384);
        assert_eq!(config.batching.min_pipeline_buffer, 100);
        assert_eq!(config.batching.batch_threshold, 4);
    }

    #[test]
    fn test_partial_toml() {
        // Only override some values, rest should be defaults
        let toml_str = r#"
            num_shards = 8
        "#;

        let config: PerformanceConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.num_shards, 8);
        assert_eq!(config.response_pool.capacity, 256); // default
        assert_eq!(config.buffers.read_size, 8192); // default
    }
}
