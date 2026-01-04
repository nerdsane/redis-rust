//! Metric command handling
//!
//! Provides custom commands for metric operations:
//! - MCOUNTER: Increment counter metrics
//! - MGAUGE: Set gauge metrics
//! - MUNIQUE: Add to unique set
//! - MQUERY: Query metric values
//! - MHOTKEYS: Get hot metrics

use super::key_encoder::MetricKeyEncoder;
use super::state::MetricsState;
use super::types::{MetricPoint, MetricType, TagSet};
use crate::production::{HotKeyDetector, HotKeyConfig};

/// Metric command types
#[derive(Debug, Clone)]
pub enum MetricsCommand {
    /// Increment a counter: MCOUNTER <name> [tag:value...] [increment]
    Counter {
        name: String,
        tags: TagSet,
        increment: i64,
    },

    /// Set a gauge: MGAUGE <name> [tag:value...] <value>
    Gauge {
        name: String,
        tags: TagSet,
        value: f64,
    },

    /// Update up-down counter: MUPDOWN <name> [tag:value...] <delta>
    UpDown {
        name: String,
        tags: TagSet,
        delta: i64,
    },

    /// Add to distribution: MDIST <name> [tag:value...] <value>
    Distribution {
        name: String,
        tags: TagSet,
        value: f64,
    },

    /// Add to unique set: MUNIQUE <name> [tag:value...] <value>
    Unique {
        name: String,
        tags: TagSet,
        value: String,
    },

    /// Query a metric: MQUERY <name> [tag:value...]
    Query { name: String, tags: TagSet },

    /// Get hot metrics: MHOTKEYS [limit]
    HotKeys { limit: usize },

    /// Get metric info: MINFO <name> [tag:value...]
    Info { name: String, tags: TagSet },

    /// List all metrics matching pattern: MLIST [pattern]
    List { pattern: Option<String> },

    /// Batch submit: MBATCH followed by metric lines
    Batch { metrics: Vec<MetricPoint> },
}

impl MetricsCommand {
    /// Parse command from RESP-style arguments
    pub fn parse(args: &[String]) -> Result<MetricsCommand, String> {
        if args.is_empty() {
            return Err("No command provided".to_string());
        }

        let cmd = args[0].to_uppercase();
        let args = &args[1..];

        match cmd.as_str() {
            "MCOUNTER" => Self::parse_counter(args),
            "MGAUGE" => Self::parse_gauge(args),
            "MUPDOWN" => Self::parse_updown(args),
            "MDIST" => Self::parse_distribution(args),
            "MUNIQUE" => Self::parse_unique(args),
            "MQUERY" => Self::parse_query(args),
            "MHOTKEYS" => Self::parse_hotkeys(args),
            "MINFO" => Self::parse_info(args),
            "MLIST" => Self::parse_list(args),
            _ => Err(format!("Unknown metric command: {}", cmd)),
        }
    }

    fn parse_counter(args: &[String]) -> Result<MetricsCommand, String> {
        if args.is_empty() {
            return Err("MCOUNTER requires metric name".to_string());
        }

        let name = args[0].clone();
        let (tags, remaining) = Self::parse_tags(&args[1..]);
        let increment = remaining
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        Ok(MetricsCommand::Counter {
            name,
            tags,
            increment,
        })
    }

    fn parse_gauge(args: &[String]) -> Result<MetricsCommand, String> {
        if args.len() < 2 {
            return Err("MGAUGE requires metric name and value".to_string());
        }

        let name = args[0].clone();
        let (tags, remaining) = Self::parse_tags(&args[1..]);
        let value = remaining
            .first()
            .ok_or("MGAUGE requires value")?
            .parse()
            .map_err(|_| "Invalid gauge value")?;

        Ok(MetricsCommand::Gauge { name, tags, value })
    }

    fn parse_updown(args: &[String]) -> Result<MetricsCommand, String> {
        if args.len() < 2 {
            return Err("MUPDOWN requires metric name and delta".to_string());
        }

        let name = args[0].clone();
        let (tags, remaining) = Self::parse_tags(&args[1..]);
        let delta = remaining
            .first()
            .ok_or("MUPDOWN requires delta")?
            .parse()
            .map_err(|_| "Invalid delta value")?;

        Ok(MetricsCommand::UpDown { name, tags, delta })
    }

    fn parse_distribution(args: &[String]) -> Result<MetricsCommand, String> {
        if args.len() < 2 {
            return Err("MDIST requires metric name and value".to_string());
        }

        let name = args[0].clone();
        let (tags, remaining) = Self::parse_tags(&args[1..]);
        let value = remaining
            .first()
            .ok_or("MDIST requires value")?
            .parse()
            .map_err(|_| "Invalid distribution value")?;

        Ok(MetricsCommand::Distribution { name, tags, value })
    }

    fn parse_unique(args: &[String]) -> Result<MetricsCommand, String> {
        if args.len() < 2 {
            return Err("MUNIQUE requires metric name and value".to_string());
        }

        let name = args[0].clone();
        let (tags, remaining) = Self::parse_tags(&args[1..]);
        let value = remaining
            .first()
            .ok_or("MUNIQUE requires value")?
            .clone();

        Ok(MetricsCommand::Unique { name, tags, value })
    }

    fn parse_query(args: &[String]) -> Result<MetricsCommand, String> {
        if args.is_empty() {
            return Err("MQUERY requires metric name".to_string());
        }

        let name = args[0].clone();
        let (tags, _) = Self::parse_tags(&args[1..]);

        Ok(MetricsCommand::Query { name, tags })
    }

    fn parse_hotkeys(args: &[String]) -> Result<MetricsCommand, String> {
        let limit = args.first().and_then(|s| s.parse().ok()).unwrap_or(10);
        Ok(MetricsCommand::HotKeys { limit })
    }

    fn parse_info(args: &[String]) -> Result<MetricsCommand, String> {
        if args.is_empty() {
            return Err("MINFO requires metric name".to_string());
        }

        let name = args[0].clone();
        let (tags, _) = Self::parse_tags(&args[1..]);

        Ok(MetricsCommand::Info { name, tags })
    }

    fn parse_list(args: &[String]) -> Result<MetricsCommand, String> {
        let pattern = args.first().cloned();
        Ok(MetricsCommand::List { pattern })
    }

    /// Parse tags from arguments (format: "key:value")
    /// Returns (TagSet, remaining_args)
    fn parse_tags(args: &[String]) -> (TagSet, Vec<String>) {
        let mut tags = std::collections::BTreeMap::new();
        let mut remaining = Vec::new();

        for arg in args {
            if arg.contains(':') && !arg.starts_with('-') {
                let parts: Vec<&str> = arg.splitn(2, ':').collect();
                if parts.len() == 2 {
                    tags.insert(parts[0].to_string(), parts[1].to_string());
                    continue;
                }
            }
            remaining.push(arg.clone());
        }

        (TagSet::new(tags), remaining)
    }
}

/// Result of executing a metric command
#[derive(Debug, Clone)]
pub enum MetricsResult {
    /// Simple OK response
    Ok,
    /// Integer value
    Integer(i64),
    /// Float value
    Float(f64),
    /// String value
    String(String),
    /// Array of values
    Array(Vec<MetricsResult>),
    /// Null/nil
    Nil,
    /// Error
    Error(String),
}

impl MetricsResult {
    /// Convert to RESP-style string
    pub fn to_resp_string(&self) -> String {
        match self {
            MetricsResult::Ok => "+OK\r\n".to_string(),
            MetricsResult::Integer(i) => format!(":{}\r\n", i),
            MetricsResult::Float(f) => format!("${}\r\n{}\r\n", f.to_string().len(), f),
            MetricsResult::String(s) => format!("${}\r\n{}\r\n", s.len(), s),
            MetricsResult::Array(arr) => {
                let mut result = format!("*{}\r\n", arr.len());
                for item in arr {
                    result.push_str(&item.to_resp_string());
                }
                result
            }
            MetricsResult::Nil => "$-1\r\n".to_string(),
            MetricsResult::Error(e) => format!("-ERR {}\r\n", e),
        }
    }
}

/// Executor for metric commands
pub struct MetricsCommandExecutor {
    state: MetricsState,
    hot_key_detector: Option<HotKeyDetector>,
}

impl MetricsCommandExecutor {
    /// Create new executor with given replica ID
    pub fn new(replica_id: u64) -> Self {
        use crate::replication::lattice::ReplicaId;
        MetricsCommandExecutor {
            state: MetricsState::new(ReplicaId::new(replica_id)),
            hot_key_detector: None,
        }
    }

    /// Create executor with hot key detection enabled
    pub fn with_hot_key_detection(mut self) -> Self {
        self.hot_key_detector = Some(HotKeyDetector::new(HotKeyConfig::default()));
        self
    }

    /// Execute a metrics command
    pub fn execute(&mut self, cmd: MetricsCommand, now_ms: u64) -> MetricsResult {
        match cmd {
            MetricsCommand::Counter {
                name,
                tags,
                increment,
            } => {
                let point = MetricPoint::counter(&name, tags.clone(), increment);
                let key = MetricKeyEncoder::encode(&name, MetricType::Counter, &tags);
                self.record_access(&key, true, now_ms);
                self.state.submit(point);
                MetricsResult::Ok
            }

            MetricsCommand::Gauge { name, tags, value } => {
                let point = MetricPoint::gauge(&name, tags.clone(), value);
                let key = MetricKeyEncoder::encode(&name, MetricType::Gauge, &tags);
                self.record_access(&key, true, now_ms);
                self.state.submit(point);
                MetricsResult::Ok
            }

            MetricsCommand::UpDown { name, tags, delta } => {
                let point = MetricPoint::up_down_counter(&name, tags.clone(), delta);
                let key = MetricKeyEncoder::encode(&name, MetricType::UpDownCounter, &tags);
                self.record_access(&key, true, now_ms);
                self.state.submit(point);
                MetricsResult::Ok
            }

            MetricsCommand::Distribution { name, tags, value } => {
                let point = MetricPoint::distribution(&name, tags.clone(), value);
                let key = MetricKeyEncoder::encode(&name, MetricType::Distribution, &tags);
                self.record_access(&key, true, now_ms);
                self.state.submit(point);
                MetricsResult::Ok
            }

            MetricsCommand::Unique { name, tags, value } => {
                let point = MetricPoint::set(&name, tags.clone(), value);
                let key = MetricKeyEncoder::encode(&name, MetricType::Set, &tags);
                self.record_access(&key, true, now_ms);
                self.state.submit(point);
                MetricsResult::Ok
            }

            MetricsCommand::Query { name, tags } => self.query_metric(&name, &tags, now_ms),

            MetricsCommand::HotKeys { limit } => self.get_hot_keys(limit, now_ms),

            MetricsCommand::Info { name, tags } => self.get_metric_info(&name, &tags),

            MetricsCommand::List { pattern } => self.list_metrics(pattern.as_deref()),

            MetricsCommand::Batch { metrics } => {
                for point in metrics {
                    self.state.submit(point);
                }
                MetricsResult::Ok
            }
        }
    }

    fn record_access(&mut self, key: &str, is_write: bool, now_ms: u64) {
        if let Some(ref mut detector) = self.hot_key_detector {
            detector.record_access(key, is_write, now_ms);
        }
    }

    fn query_metric(&mut self, name: &str, tags: &TagSet, now_ms: u64) -> MetricsResult {
        // Try each metric type
        let counter_key = MetricKeyEncoder::encode(name, MetricType::Counter, tags);
        self.record_access(&counter_key, false, now_ms);
        if let value @ 1.. = self.state.get_counter(&counter_key) {
            return MetricsResult::Integer(value as i64);
        }

        let gauge_key = MetricKeyEncoder::encode(name, MetricType::Gauge, tags);
        self.record_access(&gauge_key, false, now_ms);
        if let Some(value) = self.state.get_gauge(&gauge_key) {
            return MetricsResult::Float(value);
        }

        let updown_key = MetricKeyEncoder::encode(name, MetricType::UpDownCounter, tags);
        self.record_access(&updown_key, false, now_ms);
        let updown_value = self.state.get_up_down_counter(&updown_key);
        if updown_value != 0 {
            return MetricsResult::Integer(updown_value);
        }

        let set_key = MetricKeyEncoder::encode(name, MetricType::Set, tags);
        self.record_access(&set_key, false, now_ms);
        let cardinality = self.state.get_set_cardinality(&set_key);
        if cardinality > 0 {
            return MetricsResult::Integer(cardinality as i64);
        }

        let dist_key = MetricKeyEncoder::encode(name, MetricType::Distribution, tags);
        self.record_access(&dist_key, false, now_ms);
        if let Some(dist) = self.state.get_distribution(&dist_key) {
            return MetricsResult::Array(vec![
                MetricsResult::String("count".to_string()),
                MetricsResult::Integer(dist.count as i64),
                MetricsResult::String("avg".to_string()),
                MetricsResult::Float(dist.avg()),
                MetricsResult::String("min".to_string()),
                MetricsResult::Float(dist.min),
                MetricsResult::String("max".to_string()),
                MetricsResult::Float(dist.max),
                MetricsResult::String("p50".to_string()),
                MetricsResult::Float(dist.p50()),
                MetricsResult::String("p90".to_string()),
                MetricsResult::Float(dist.p90()),
                MetricsResult::String("p99".to_string()),
                MetricsResult::Float(dist.p99()),
            ]);
        }

        MetricsResult::Nil
    }

    fn get_hot_keys(&self, limit: usize, now_ms: u64) -> MetricsResult {
        match &self.hot_key_detector {
            Some(detector) => {
                let hot_keys = detector.get_top_keys(limit, now_ms);
                let results: Vec<MetricsResult> = hot_keys
                    .into_iter()
                    .flat_map(|(key, rate)| {
                        vec![MetricsResult::String(key), MetricsResult::Float(rate)]
                    })
                    .collect();
                MetricsResult::Array(results)
            }
            None => MetricsResult::Array(vec![]),
        }
    }

    fn get_metric_info(&self, name: &str, tags: &TagSet) -> MetricsResult {
        let mut info = Vec::new();

        // Check each type
        let counter_key = MetricKeyEncoder::encode(name, MetricType::Counter, tags);
        let counter_val = self.state.get_counter(&counter_key);
        if counter_val > 0 {
            info.push(MetricsResult::String("type".to_string()));
            info.push(MetricsResult::String("counter".to_string()));
            info.push(MetricsResult::String("value".to_string()));
            info.push(MetricsResult::Integer(counter_val as i64));
        }

        let gauge_key = MetricKeyEncoder::encode(name, MetricType::Gauge, tags);
        if let Some(value) = self.state.get_gauge(&gauge_key) {
            info.push(MetricsResult::String("type".to_string()));
            info.push(MetricsResult::String("gauge".to_string()));
            info.push(MetricsResult::String("value".to_string()));
            info.push(MetricsResult::Float(value));
        }

        if info.is_empty() {
            MetricsResult::Nil
        } else {
            info.push(MetricsResult::String("tags".to_string()));
            info.push(MetricsResult::String(tags.to_string()));
            MetricsResult::Array(info)
        }
    }

    fn list_metrics(&self, pattern: Option<&str>) -> MetricsResult {
        let keys = self.state.keys();
        let filtered: Vec<String> = match pattern {
            Some(p) => keys.into_iter().filter(|k| k.contains(p)).collect(),
            None => keys,
        };

        MetricsResult::Array(filtered.into_iter().map(MetricsResult::String).collect())
    }

    /// Get the underlying state for replication
    pub fn state(&self) -> &MetricsState {
        &self.state
    }

    /// Get mutable state
    pub fn state_mut(&mut self) -> &mut MetricsState {
        &mut self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_counter() {
        let args = vec![
            "MCOUNTER".to_string(),
            "http.requests".to_string(),
            "host:web01".to_string(),
            "env:prod".to_string(),
            "100".to_string(),
        ];

        let cmd = MetricsCommand::parse(&args).unwrap();
        match cmd {
            MetricsCommand::Counter {
                name,
                tags,
                increment,
            } => {
                assert_eq!(name, "http.requests");
                assert_eq!(tags.get("host"), Some(&"web01".to_string()));
                assert_eq!(tags.get("env"), Some(&"prod".to_string()));
                assert_eq!(increment, 100);
            }
            _ => panic!("Expected Counter command"),
        }
    }

    #[test]
    fn test_parse_gauge() {
        let args = vec![
            "MGAUGE".to_string(),
            "system.cpu".to_string(),
            "host:web01".to_string(),
            "75.5".to_string(),
        ];

        let cmd = MetricsCommand::parse(&args).unwrap();
        match cmd {
            MetricsCommand::Gauge { name, tags, value } => {
                assert_eq!(name, "system.cpu");
                assert_eq!(tags.get("host"), Some(&"web01".to_string()));
                assert_eq!(value, 75.5);
            }
            _ => panic!("Expected Gauge command"),
        }
    }

    #[test]
    fn test_execute_counter() {
        let mut executor = MetricsCommandExecutor::new(1);

        let result = executor.execute(
            MetricsCommand::Counter {
                name: "http.requests".to_string(),
                tags: TagSet::from_pairs(&[("host", "web01")]),
                increment: 100,
            },
            0,
        );

        assert!(matches!(result, MetricsResult::Ok));

        // Query the value
        let result = executor.execute(
            MetricsCommand::Query {
                name: "http.requests".to_string(),
                tags: TagSet::from_pairs(&[("host", "web01")]),
            },
            0,
        );

        match result {
            MetricsResult::Integer(v) => assert_eq!(v, 100),
            _ => panic!("Expected Integer result"),
        }
    }

    #[test]
    fn test_execute_gauge() {
        let mut executor = MetricsCommandExecutor::new(1);

        executor.execute(
            MetricsCommand::Gauge {
                name: "system.cpu".to_string(),
                tags: TagSet::from_pairs(&[("host", "web01")]),
                value: 75.5,
            },
            0,
        );

        let result = executor.execute(
            MetricsCommand::Query {
                name: "system.cpu".to_string(),
                tags: TagSet::from_pairs(&[("host", "web01")]),
            },
            0,
        );

        match result {
            MetricsResult::Float(v) => assert!((v - 75.5).abs() < 0.001),
            _ => panic!("Expected Float result"),
        }
    }

    #[test]
    fn test_hot_key_detection() {
        let mut executor = MetricsCommandExecutor::new(1).with_hot_key_detection();

        // Query same metric many times
        for i in 0..200 {
            executor.execute(
                MetricsCommand::Counter {
                    name: "hot.metric".to_string(),
                    tags: TagSet::from_pairs(&[("host", "web01")]),
                    increment: 1,
                },
                i * 5, // 5ms apart = 200 ops/sec
            );
        }

        let result = executor.execute(MetricsCommand::HotKeys { limit: 10 }, 1000);

        match result {
            MetricsResult::Array(arr) => {
                assert!(!arr.is_empty(), "Should have detected hot keys");
            }
            _ => panic!("Expected Array result"),
        }
    }
}
