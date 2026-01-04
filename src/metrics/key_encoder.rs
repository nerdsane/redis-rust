//! Key encoding for metrics storage
//!
//! Encodes metric names, types, and tags into Redis keys for efficient storage and querying.
//!
//! Key format: `metric:<type>:<name>:<tags_hash>`
//!
//! Examples:
//! - `metric:c:http.requests:a3b2c1d4e5f6a7b8` (counter)
//! - `metric:g:system.cpu.load:1234567890abcdef` (gauge)

use super::types::{MetricType, TagSet};

/// Encodes and decodes metric keys for Redis storage
pub struct MetricKeyEncoder;

impl MetricKeyEncoder {
    /// Encode a metric into a Redis key
    ///
    /// Format: `metric:<type>:<name>:<tags_hash>`
    pub fn encode(name: &str, metric_type: MetricType, tags: &TagSet) -> String {
        format!(
            "metric:{}:{}:{:016x}",
            metric_type.type_code(),
            name,
            tags.hash()
        )
    }

    /// Encode a metric metadata key (stores tag definitions)
    ///
    /// Format: `meta:<name>:<tags_hash>`
    pub fn encode_meta(name: &str, tags: &TagSet) -> String {
        format!("meta:{}:{:016x}", name, tags.hash())
    }

    /// Encode a time-bucketed key for aggregation
    ///
    /// Format: `bucket:<resolution>:<name>:<tags_hash>:<timestamp_bucket>`
    pub fn encode_bucket(
        name: &str,
        tags: &TagSet,
        resolution_secs: u64,
        timestamp_ms: u64,
    ) -> String {
        let bucket = (timestamp_ms / 1000) / resolution_secs * resolution_secs;
        format!(
            "bucket:{}s:{}:{:016x}:{}",
            resolution_secs,
            name,
            tags.hash(),
            bucket
        )
    }

    /// Encode a distribution data key (for histogram/percentile data)
    ///
    /// Format: `dist:<name>:<tags_hash>`
    pub fn encode_distribution(name: &str, tags: &TagSet) -> String {
        format!("dist:{}:{:016x}", name, tags.hash())
    }

    /// Decode a metric key back to components
    ///
    /// Returns: (name, metric_type, tags_hash)
    pub fn decode(key: &str) -> Option<(String, MetricType, u64)> {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() < 4 || parts[0] != "metric" {
            return None;
        }

        let type_code = parts[1].chars().next()?;
        let metric_type = MetricType::from_type_code(type_code)?;
        let name = parts[2].to_string();
        let tags_hash = u64::from_str_radix(parts[3], 16).ok()?;

        Some((name, metric_type, tags_hash))
    }

    /// Check if a key is a metric key
    pub fn is_metric_key(key: &str) -> bool {
        key.starts_with("metric:")
    }

    /// Check if a key is a metadata key
    pub fn is_meta_key(key: &str) -> bool {
        key.starts_with("meta:")
    }

    /// Check if a key is a bucket key
    pub fn is_bucket_key(key: &str) -> bool {
        key.starts_with("bucket:")
    }

    /// Check if a key is a distribution key
    pub fn is_distribution_key(key: &str) -> bool {
        key.starts_with("dist:")
    }

    /// Generate a pattern for finding all metrics of a given name
    ///
    /// Returns: `metric:*:<name>:*`
    pub fn name_pattern(name: &str) -> String {
        format!("metric:*:{}:*", name)
    }

    /// Generate a pattern for finding all metrics of a given type
    ///
    /// Returns: `metric:<type>:*:*`
    pub fn type_pattern(metric_type: MetricType) -> String {
        format!("metric:{}:*:*", metric_type.type_code())
    }

    /// Generate a pattern for finding all counters
    pub fn counter_pattern() -> String {
        "metric:c:*:*".to_string()
    }

    /// Generate a pattern for finding all gauges
    pub fn gauge_pattern() -> String {
        "metric:g:*:*".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_counter() {
        let tags = TagSet::from_pairs(&[("host", "web01"), ("env", "prod")]);
        let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);

        assert!(key.starts_with("metric:c:http.requests:"));
        assert_eq!(key.len(), "metric:c:http.requests:".len() + 16); // 16 hex chars
    }

    #[test]
    fn test_encode_gauge() {
        let tags = TagSet::from_pairs(&[("host", "web01")]);
        let key = MetricKeyEncoder::encode("system.cpu", MetricType::Gauge, &tags);

        assert!(key.starts_with("metric:g:system.cpu:"));
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let tags = TagSet::from_pairs(&[("host", "web01")]);
        let original_name = "http.requests";
        let original_type = MetricType::Counter;

        let key = MetricKeyEncoder::encode(original_name, original_type, &tags);
        let (name, metric_type, tags_hash) = MetricKeyEncoder::decode(&key).unwrap();

        assert_eq!(name, original_name);
        assert_eq!(metric_type, original_type);
        assert_eq!(tags_hash, tags.hash());
    }

    #[test]
    fn test_decode_invalid_key() {
        assert!(MetricKeyEncoder::decode("invalid").is_none());
        assert!(MetricKeyEncoder::decode("not:a:metric:key").is_none());
        assert!(MetricKeyEncoder::decode("metric:x:name:0000").is_none()); // Invalid type
    }

    #[test]
    fn test_encode_meta() {
        let tags = TagSet::from_pairs(&[("host", "web01")]);
        let key = MetricKeyEncoder::encode_meta("http.requests", &tags);

        assert!(key.starts_with("meta:http.requests:"));
    }

    #[test]
    fn test_encode_bucket() {
        let tags = TagSet::from_pairs(&[("host", "web01")]);
        // 10-second resolution, timestamp 1704067215000 ms
        let key = MetricKeyEncoder::encode_bucket("http.requests", &tags, 10, 1704067215000);

        // Bucket should be floored to 10-second boundary: 1704067210
        assert!(key.contains(":1704067210"));
        assert!(key.starts_with("bucket:10s:http.requests:"));
    }

    #[test]
    fn test_key_type_detection() {
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        let metric_key = MetricKeyEncoder::encode("test", MetricType::Counter, &tags);
        assert!(MetricKeyEncoder::is_metric_key(&metric_key));
        assert!(!MetricKeyEncoder::is_meta_key(&metric_key));

        let meta_key = MetricKeyEncoder::encode_meta("test", &tags);
        assert!(MetricKeyEncoder::is_meta_key(&meta_key));
        assert!(!MetricKeyEncoder::is_metric_key(&meta_key));
    }

    #[test]
    fn test_patterns() {
        assert_eq!(
            MetricKeyEncoder::name_pattern("http.requests"),
            "metric:*:http.requests:*"
        );
        assert_eq!(
            MetricKeyEncoder::type_pattern(MetricType::Counter),
            "metric:c:*:*"
        );
        assert_eq!(MetricKeyEncoder::counter_pattern(), "metric:c:*:*");
        assert_eq!(MetricKeyEncoder::gauge_pattern(), "metric:g:*:*");
    }

    #[test]
    fn test_same_tags_same_hash() {
        // Same tags in different order should produce same key
        let tags1 = TagSet::from_pairs(&[("a", "1"), ("b", "2")]);
        let tags2 = TagSet::from_pairs(&[("b", "2"), ("a", "1")]);

        let key1 = MetricKeyEncoder::encode("test", MetricType::Counter, &tags1);
        let key2 = MetricKeyEncoder::encode("test", MetricType::Counter, &tags2);

        assert_eq!(key1, key2);
    }

    #[test]
    fn test_different_tags_different_hash() {
        let tags1 = TagSet::from_pairs(&[("host", "web01")]);
        let tags2 = TagSet::from_pairs(&[("host", "web02")]);

        let key1 = MetricKeyEncoder::encode("test", MetricType::Counter, &tags1);
        let key2 = MetricKeyEncoder::encode("test", MetricType::Counter, &tags2);

        assert_ne!(key1, key2);
    }
}
