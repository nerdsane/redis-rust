//! Datadog Configuration
//!
//! All settings are loaded from environment variables for 12-factor compliance.
//! Uses standard Datadog environment variable names (DD_*).

use std::net::SocketAddr;

/// Datadog configuration loaded from environment variables
#[derive(Debug, Clone)]
pub struct DatadogConfig {
    /// DogStatsD agent address (default: 127.0.0.1:8125)
    pub statsd_addr: SocketAddr,
    /// Trace agent address (default: 127.0.0.1:8126)
    pub trace_addr: String,
    /// Service name for APM (default: redis-rust)
    pub service_name: String,
    /// Environment tag (default: development)
    pub env: String,
    /// Service version (default: from Cargo.toml)
    pub version: String,
    /// Sample rate for traces (0.0 - 1.0, default: 1.0)
    pub trace_sample_rate: f64,
    /// Enable JSON logs with trace correlation
    pub logs_injection: bool,
    /// Metric prefix (default: redis_rust)
    pub metric_prefix: String,
    /// Additional global tags (parsed from DD_TAGS)
    pub global_tags: Vec<(String, String)>,
}

impl Default for DatadogConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl DatadogConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        DatadogConfig {
            statsd_addr: std::env::var("DD_DOGSTATSD_URL")
                .unwrap_or_else(|_| "127.0.0.1:8125".to_string())
                .parse()
                .unwrap_or_else(|_| "127.0.0.1:8125".parse().expect("hardcoded address must parse")),
            trace_addr: std::env::var("DD_TRACE_AGENT_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8126".to_string()),
            service_name: std::env::var("DD_SERVICE").unwrap_or_else(|_| "redis-rust".to_string()),
            env: std::env::var("DD_ENV").unwrap_or_else(|_| "development".to_string()),
            version: std::env::var("DD_VERSION")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
            trace_sample_rate: std::env::var("DD_TRACE_SAMPLE_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
            logs_injection: std::env::var("DD_LOGS_INJECTION")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            metric_prefix: std::env::var("DD_METRIC_PREFIX")
                .unwrap_or_else(|_| "redis_rust".to_string()),
            global_tags: Self::parse_global_tags(),
        }
    }

    /// Parse DD_TAGS environment variable (format: "key1:value1,key2:value2")
    fn parse_global_tags() -> Vec<(String, String)> {
        std::env::var("DD_TAGS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|tag| {
                let parts: Vec<&str> = tag.splitn(2, ':').collect();
                if parts.len() == 2 {
                    Some((parts[0].trim().to_string(), parts[1].trim().to_string()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get all tags as formatted strings for DogStatsD
    pub fn formatted_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .global_tags
            .iter()
            .map(|(k, v)| format!("{}:{}", k, v))
            .collect();

        tags.push(format!("env:{}", self.env));
        tags.push(format!("service:{}", self.service_name));
        tags.push(format!("version:{}", self.version));

        tags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DatadogConfig::from_env();
        assert_eq!(config.service_name, "redis-rust");
        assert_eq!(config.metric_prefix, "redis_rust");
        assert!((config.trace_sample_rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_tags() {
        std::env::set_var("DD_TAGS", "region:us-east-1,cluster:primary");
        let tags = DatadogConfig::parse_global_tags();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], ("region".to_string(), "us-east-1".to_string()));
        assert_eq!(tags[1], ("cluster".to_string(), "primary".to_string()));
        std::env::remove_var("DD_TAGS");
    }
}
