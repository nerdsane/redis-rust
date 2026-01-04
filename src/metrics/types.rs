//! Core metric types for the aggregation service

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Type of metric being tracked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetricType {
    /// Monotonically increasing counter (uses GCounter CRDT)
    /// Use for: request counts, page views, error counts
    Counter,

    /// Point-in-time value (uses LwwRegister CRDT)
    /// Use for: CPU load, memory usage, temperature
    Gauge,

    /// Bidirectional counter (uses PNCounter CRDT)
    /// Use for: active connections, queue depth
    UpDownCounter,

    /// Distribution/histogram for percentiles
    /// Use for: latency, request sizes
    Distribution,

    /// Unique value set (uses ORSet CRDT)
    /// Use for: unique users, unique errors
    Set,
}

impl MetricType {
    /// Get the single-character type code for key encoding
    pub fn type_code(&self) -> char {
        match self {
            MetricType::Counter => 'c',
            MetricType::Gauge => 'g',
            MetricType::UpDownCounter => 'u',
            MetricType::Distribution => 'd',
            MetricType::Set => 's',
        }
    }

    /// Parse type code back to MetricType
    pub fn from_type_code(code: char) -> Option<MetricType> {
        match code {
            'c' => Some(MetricType::Counter),
            'g' => Some(MetricType::Gauge),
            'u' => Some(MetricType::UpDownCounter),
            'd' => Some(MetricType::Distribution),
            's' => Some(MetricType::Set),
            _ => None,
        }
    }
}

/// A set of tags (key-value pairs) associated with a metric
/// Tags are stored in sorted order for deterministic hashing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagSet {
    tags: BTreeMap<String, String>,
    hash: u64,
}

impl TagSet {
    /// Create a new TagSet from key-value pairs
    pub fn new(tags: BTreeMap<String, String>) -> Self {
        let hash = Self::compute_hash(&tags);
        TagSet { tags, hash }
    }

    /// Create an empty TagSet
    pub fn empty() -> Self {
        TagSet {
            tags: BTreeMap::new(),
            hash: 0,
        }
    }

    /// Create TagSet from slice of (key, value) tuples
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        let tags: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self::new(tags)
    }

    /// Parse tags from "key:value,key2:value2" format
    pub fn parse(s: &str) -> Self {
        if s.is_empty() {
            return Self::empty();
        }

        let tags: BTreeMap<String, String> = s
            .split(',')
            .filter_map(|pair| {
                let mut parts = pair.splitn(2, ':');
                let key = parts.next()?.trim();
                let value = parts.next()?.trim();
                if key.is_empty() || value.is_empty() {
                    None
                } else {
                    Some((key.to_string(), value.to_string()))
                }
            })
            .collect();
        Self::new(tags)
    }

    /// Get the precomputed hash
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Get the underlying tags
    pub fn tags(&self) -> &BTreeMap<String, String> {
        &self.tags
    }

    /// Get a specific tag value
    pub fn get(&self, key: &str) -> Option<&String> {
        self.tags.get(key)
    }

    /// Check if tags match a pattern (supports wildcards)
    pub fn matches(&self, pattern: &TagSet) -> bool {
        for (key, pattern_value) in &pattern.tags {
            match self.tags.get(key) {
                Some(value) => {
                    if pattern_value != "*" && value != pattern_value {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }

    /// Serialize tags to "key:value,key2:value2" format
    pub fn to_string(&self) -> String {
        self.tags
            .iter()
            .map(|(k, v)| format!("{}:{}", k, v))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Compute hash from sorted tags
    fn compute_hash(tags: &BTreeMap<String, String>) -> u64 {
        let mut hasher = DefaultHasher::new();
        for (k, v) in tags {
            k.hash(&mut hasher);
            v.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Number of tags
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

impl Default for TagSet {
    fn default() -> Self {
        Self::empty()
    }
}

/// Value associated with a metric submission
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetricValue {
    /// Integer value (for counters)
    Integer(i64),
    /// Floating point value (for gauges, distributions)
    Float(f64),
    /// String value (for sets - unique tracking)
    String(String),
}

impl MetricValue {
    /// Get as i64, converting if necessary
    pub fn as_i64(&self) -> i64 {
        match self {
            MetricValue::Integer(i) => *i,
            MetricValue::Float(f) => *f as i64,
            MetricValue::String(_) => 0,
        }
    }

    /// Get as f64, converting if necessary
    pub fn as_f64(&self) -> f64 {
        match self {
            MetricValue::Integer(i) => *i as f64,
            MetricValue::Float(f) => *f,
            MetricValue::String(_) => 0.0,
        }
    }

    /// Get as string
    pub fn as_string(&self) -> String {
        match self {
            MetricValue::Integer(i) => i.to_string(),
            MetricValue::Float(f) => f.to_string(),
            MetricValue::String(s) => s.clone(),
        }
    }
}

impl From<i64> for MetricValue {
    fn from(v: i64) -> Self {
        MetricValue::Integer(v)
    }
}

impl From<f64> for MetricValue {
    fn from(v: f64) -> Self {
        MetricValue::Float(v)
    }
}

impl From<String> for MetricValue {
    fn from(v: String) -> Self {
        MetricValue::String(v)
    }
}

impl From<&str> for MetricValue {
    fn from(v: &str) -> Self {
        MetricValue::String(v.to_string())
    }
}

/// A single metric data point
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricPoint {
    /// Metric name (e.g., "http.requests", "system.cpu.load")
    pub name: String,

    /// Type of metric
    pub metric_type: MetricType,

    /// Tags associated with this metric
    pub tags: TagSet,

    /// The value being submitted
    pub value: MetricValue,

    /// Timestamp in milliseconds (0 = server assigns timestamp)
    pub timestamp_ms: u64,
}

impl MetricPoint {
    /// Create a new counter metric point
    pub fn counter(name: impl Into<String>, tags: TagSet, value: i64) -> Self {
        MetricPoint {
            name: name.into(),
            metric_type: MetricType::Counter,
            tags,
            value: MetricValue::Integer(value),
            timestamp_ms: 0,
        }
    }

    /// Create a new gauge metric point
    pub fn gauge(name: impl Into<String>, tags: TagSet, value: f64) -> Self {
        MetricPoint {
            name: name.into(),
            metric_type: MetricType::Gauge,
            tags,
            value: MetricValue::Float(value),
            timestamp_ms: 0,
        }
    }

    /// Create a new up-down counter metric point
    pub fn up_down_counter(name: impl Into<String>, tags: TagSet, value: i64) -> Self {
        MetricPoint {
            name: name.into(),
            metric_type: MetricType::UpDownCounter,
            tags,
            value: MetricValue::Integer(value),
            timestamp_ms: 0,
        }
    }

    /// Create a new distribution metric point
    pub fn distribution(name: impl Into<String>, tags: TagSet, value: f64) -> Self {
        MetricPoint {
            name: name.into(),
            metric_type: MetricType::Distribution,
            tags,
            value: MetricValue::Float(value),
            timestamp_ms: 0,
        }
    }

    /// Create a new set metric point (for unique tracking)
    pub fn set(name: impl Into<String>, tags: TagSet, value: impl Into<String>) -> Self {
        MetricPoint {
            name: name.into(),
            metric_type: MetricType::Set,
            tags,
            value: MetricValue::String(value.into()),
            timestamp_ms: 0,
        }
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, timestamp_ms: u64) -> Self {
        self.timestamp_ms = timestamp_ms;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tag_set_creation() {
        let tags = TagSet::from_pairs(&[("host", "web01"), ("env", "prod")]);
        assert_eq!(tags.get("host"), Some(&"web01".to_string()));
        assert_eq!(tags.get("env"), Some(&"prod".to_string()));
        assert_eq!(tags.get("missing"), None);
    }

    #[test]
    fn test_tag_set_parsing() {
        let tags = TagSet::parse("host:web01,env:prod,service:api");
        assert_eq!(tags.len(), 3);
        assert_eq!(tags.get("host"), Some(&"web01".to_string()));
        assert_eq!(tags.get("env"), Some(&"prod".to_string()));
        assert_eq!(tags.get("service"), Some(&"api".to_string()));
    }

    #[test]
    fn test_tag_set_hash_deterministic() {
        let tags1 = TagSet::from_pairs(&[("a", "1"), ("b", "2")]);
        let tags2 = TagSet::from_pairs(&[("b", "2"), ("a", "1")]); // Different order
        // BTreeMap ensures sorted order, so hashes should match
        assert_eq!(tags1.hash(), tags2.hash());
    }

    #[test]
    fn test_tag_set_matching() {
        let tags = TagSet::from_pairs(&[("host", "web01"), ("env", "prod")]);

        // Exact match
        let pattern1 = TagSet::from_pairs(&[("host", "web01")]);
        assert!(tags.matches(&pattern1));

        // Wildcard match
        let pattern2 = TagSet::from_pairs(&[("host", "*")]);
        assert!(tags.matches(&pattern2));

        // Non-match
        let pattern3 = TagSet::from_pairs(&[("host", "web02")]);
        assert!(!tags.matches(&pattern3));
    }

    #[test]
    fn test_metric_type_codes() {
        assert_eq!(MetricType::Counter.type_code(), 'c');
        assert_eq!(MetricType::Gauge.type_code(), 'g');
        assert_eq!(MetricType::from_type_code('c'), Some(MetricType::Counter));
        assert_eq!(MetricType::from_type_code('x'), None);
    }

    #[test]
    fn test_metric_point_creation() {
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        let counter = MetricPoint::counter("http.requests", tags.clone(), 100);
        assert_eq!(counter.metric_type, MetricType::Counter);
        assert_eq!(counter.value.as_i64(), 100);

        let gauge = MetricPoint::gauge("system.cpu", tags.clone(), 75.5);
        assert_eq!(gauge.metric_type, MetricType::Gauge);
        assert_eq!(gauge.value.as_f64(), 75.5);
    }

    #[test]
    fn test_metric_value_conversions() {
        let int_val = MetricValue::Integer(42);
        assert_eq!(int_val.as_i64(), 42);
        assert_eq!(int_val.as_f64(), 42.0);

        let float_val = MetricValue::Float(3.14);
        assert_eq!(float_val.as_i64(), 3);
        assert_eq!(float_val.as_f64(), 3.14);

        let str_val = MetricValue::String("test".to_string());
        assert_eq!(str_val.as_string(), "test");
    }
}
