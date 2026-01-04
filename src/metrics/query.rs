//! Metric query and aggregation
//!
//! Provides filtering and aggregation capabilities for metrics.

use std::collections::HashMap;

use super::key_encoder::MetricKeyEncoder;
use super::state::MetricsState;
use super::types::{MetricType, TagSet};

/// Type of aggregation to perform
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationType {
    /// Sum all values
    Sum,
    /// Average of values
    Avg,
    /// Minimum value
    Min,
    /// Maximum value
    Max,
    /// Count of data points
    Count,
    /// 50th percentile
    P50,
    /// 90th percentile
    P90,
    /// 99th percentile
    P99,
}

/// A query for metrics
#[derive(Debug, Clone)]
pub struct MetricsQuery {
    /// Metric name (supports wildcards with *)
    pub name: String,
    /// Tag filters (supports wildcards with *)
    pub tag_filters: TagSet,
    /// Aggregation type
    pub aggregation: Option<AggregationType>,
    /// Group by tags
    pub group_by: Vec<String>,
    /// Time range start (ms)
    pub from_ms: Option<u64>,
    /// Time range end (ms)
    pub to_ms: Option<u64>,
}

impl MetricsQuery {
    /// Create a new query for a metric name
    pub fn new(name: impl Into<String>) -> Self {
        MetricsQuery {
            name: name.into(),
            tag_filters: TagSet::empty(),
            aggregation: None,
            group_by: Vec::new(),
            from_ms: None,
            to_ms: None,
        }
    }

    /// Add tag filters
    pub fn with_tags(mut self, tags: TagSet) -> Self {
        self.tag_filters = tags;
        self
    }

    /// Add a single tag filter
    pub fn filter_tag(mut self, key: &str, value: &str) -> Self {
        let mut tags = self.tag_filters.tags().clone();
        tags.insert(key.to_string(), value.to_string());
        self.tag_filters = TagSet::new(tags);
        self
    }

    /// Set aggregation type
    pub fn aggregate(mut self, agg: AggregationType) -> Self {
        self.aggregation = Some(agg);
        self
    }

    /// Group results by tag
    pub fn group_by_tag(mut self, tag: &str) -> Self {
        self.group_by.push(tag.to_string());
        self
    }

    /// Set time range
    pub fn time_range(mut self, from_ms: u64, to_ms: u64) -> Self {
        self.from_ms = Some(from_ms);
        self.to_ms = Some(to_ms);
        self
    }
}

/// Result of a metric query
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// The metric name
    pub name: String,
    /// Tags for this result
    pub tags: TagSet,
    /// The value
    pub value: QueryValue,
}

/// Value types for query results
#[derive(Debug, Clone)]
pub enum QueryValue {
    /// Integer value (counters)
    Integer(i64),
    /// Float value (gauges, aggregations)
    Float(f64),
    /// Distribution statistics
    Distribution {
        count: u64,
        sum: f64,
        avg: f64,
        min: f64,
        max: f64,
        p50: f64,
        p90: f64,
        p99: f64,
    },
    /// Set cardinality
    Cardinality(usize),
    /// No value found
    None,
}

/// Query executor
pub struct QueryExecutor<'a> {
    state: &'a MetricsState,
}

impl<'a> QueryExecutor<'a> {
    /// Create a new query executor
    pub fn new(state: &'a MetricsState) -> Self {
        QueryExecutor { state }
    }

    /// Execute a query
    pub fn execute(&self, query: &MetricsQuery) -> Vec<QueryResult> {
        let mut results = Vec::new();

        // Get all keys that match the query
        let keys = self.state.keys();

        for key in keys {
            // Parse key to check if it matches
            if let Some((name, metric_type, tags_hash)) = MetricKeyEncoder::decode(&key) {
                // Check name match (simple contains for now, could add wildcard support)
                if !self.name_matches(&name, &query.name) {
                    continue;
                }

                // Get value based on type
                let value = self.get_value(&key, metric_type);

                // Create result with empty tags (we'd need to store tag metadata to restore)
                results.push(QueryResult {
                    name,
                    tags: TagSet::empty(), // TODO: restore from metadata
                    value,
                });
            }
        }

        // Apply aggregation if specified
        if let Some(agg) = query.aggregation {
            results = self.aggregate_results(results, agg);
        }

        results
    }

    fn name_matches(&self, name: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if pattern.contains('*') {
            // Simple wildcard matching
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                let (prefix, suffix) = (parts[0], parts[1]);
                return name.starts_with(prefix) && name.ends_with(suffix);
            }
        }
        name == pattern
    }

    fn get_value(&self, key: &str, metric_type: MetricType) -> QueryValue {
        match metric_type {
            MetricType::Counter => {
                let value = self.state.get_counter(key);
                QueryValue::Integer(value as i64)
            }
            MetricType::Gauge => match self.state.get_gauge(key) {
                Some(v) => QueryValue::Float(v),
                None => QueryValue::None,
            },
            MetricType::UpDownCounter => {
                let value = self.state.get_up_down_counter(key);
                QueryValue::Integer(value)
            }
            MetricType::Set => {
                let card = self.state.get_set_cardinality(key);
                QueryValue::Cardinality(card)
            }
            MetricType::Distribution => {
                match self.state.get_distribution(key) {
                    Some(dist) => QueryValue::Distribution {
                        count: dist.count,
                        sum: dist.sum,
                        avg: dist.avg(),
                        min: dist.min,
                        max: dist.max,
                        p50: dist.p50(),
                        p90: dist.p90(),
                        p99: dist.p99(),
                    },
                    None => QueryValue::None,
                }
            }
        }
    }

    fn aggregate_results(
        &self,
        results: Vec<QueryResult>,
        agg: AggregationType,
    ) -> Vec<QueryResult> {
        if results.is_empty() {
            return results;
        }

        // Collect all numeric values
        let values: Vec<f64> = results
            .iter()
            .filter_map(|r| match &r.value {
                QueryValue::Integer(i) => Some(*i as f64),
                QueryValue::Float(f) => Some(*f),
                QueryValue::Cardinality(c) => Some(*c as f64),
                _ => None,
            })
            .collect();

        if values.is_empty() {
            return results;
        }

        let aggregated_value = match agg {
            AggregationType::Sum => values.iter().sum(),
            AggregationType::Avg => values.iter().sum::<f64>() / values.len() as f64,
            AggregationType::Min => values.iter().cloned().fold(f64::MAX, f64::min),
            AggregationType::Max => values.iter().cloned().fold(f64::MIN, f64::max),
            AggregationType::Count => values.len() as f64,
            AggregationType::P50 => self.percentile(&values, 50.0),
            AggregationType::P90 => self.percentile(&values, 90.0),
            AggregationType::P99 => self.percentile(&values, 99.0),
        };

        vec![QueryResult {
            name: results[0].name.clone(),
            tags: TagSet::empty(),
            value: QueryValue::Float(aggregated_value),
        }]
    }

    fn percentile(&self, values: &[f64], p: f64) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let idx = ((p / 100.0) * (sorted.len() - 1) as f64) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }
}

/// Builder for grouped queries (for dashboards)
pub struct DashboardQuery {
    queries: Vec<MetricsQuery>,
}

impl DashboardQuery {
    /// Create a new dashboard query builder
    pub fn new() -> Self {
        DashboardQuery {
            queries: Vec::new(),
        }
    }

    /// Add a query to the dashboard
    pub fn add_query(mut self, query: MetricsQuery) -> Self {
        self.queries.push(query);
        self
    }

    /// Execute all queries
    pub fn execute<'a>(&self, state: &'a MetricsState) -> HashMap<String, Vec<QueryResult>> {
        let executor = QueryExecutor::new(state);
        let mut results = HashMap::new();

        for query in &self.queries {
            let name = query.name.clone();
            let query_results = executor.execute(query);
            results.insert(name, query_results);
        }

        results
    }
}

impl Default for DashboardQuery {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::types::MetricPoint;
    use crate::replication::lattice::ReplicaId;

    #[test]
    fn test_simple_query() {
        let mut state = MetricsState::new(ReplicaId::new(1));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        state.submit(MetricPoint::counter("http.requests", tags.clone(), 100));

        let executor = QueryExecutor::new(&state);
        let query = MetricsQuery::new("http.requests");
        let results = executor.execute(&query);

        assert_eq!(results.len(), 1);
        match &results[0].value {
            QueryValue::Integer(v) => assert_eq!(*v, 100),
            _ => panic!("Expected Integer"),
        }
    }

    #[test]
    fn test_wildcard_query() {
        let mut state = MetricsState::new(ReplicaId::new(1));

        state.submit(MetricPoint::counter(
            "http.requests",
            TagSet::from_pairs(&[("host", "web01")]),
            100,
        ));
        state.submit(MetricPoint::counter(
            "http.errors",
            TagSet::from_pairs(&[("host", "web01")]),
            10,
        ));
        state.submit(MetricPoint::counter(
            "db.queries",
            TagSet::from_pairs(&[("host", "db01")]),
            50,
        ));

        let executor = QueryExecutor::new(&state);
        let query = MetricsQuery::new("http.*");
        let results = executor.execute(&query);

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_sum_aggregation() {
        let mut state = MetricsState::new(ReplicaId::new(1));

        state.submit(MetricPoint::counter(
            "http.requests",
            TagSet::from_pairs(&[("host", "web01")]),
            100,
        ));
        state.submit(MetricPoint::counter(
            "http.requests",
            TagSet::from_pairs(&[("host", "web02")]),
            200,
        ));

        let executor = QueryExecutor::new(&state);
        let query = MetricsQuery::new("http.requests").aggregate(AggregationType::Sum);
        let results = executor.execute(&query);

        assert_eq!(results.len(), 1);
        match &results[0].value {
            QueryValue::Float(v) => assert_eq!(*v, 300.0),
            _ => panic!("Expected Float"),
        }
    }

    #[test]
    fn test_avg_aggregation() {
        let mut state = MetricsState::new(ReplicaId::new(1));

        state.submit(MetricPoint::gauge(
            "system.cpu",
            TagSet::from_pairs(&[("host", "web01")]),
            50.0,
        ));
        state.submit(MetricPoint::gauge(
            "system.cpu",
            TagSet::from_pairs(&[("host", "web02")]),
            70.0,
        ));

        let executor = QueryExecutor::new(&state);
        let query = MetricsQuery::new("system.cpu").aggregate(AggregationType::Avg);
        let results = executor.execute(&query);

        assert_eq!(results.len(), 1);
        match &results[0].value {
            QueryValue::Float(v) => assert!((v - 60.0).abs() < 0.001),
            _ => panic!("Expected Float"),
        }
    }

    #[test]
    fn test_dashboard_query() {
        let mut state = MetricsState::new(ReplicaId::new(1));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        state.submit(MetricPoint::counter("http.requests", tags.clone(), 100));
        state.submit(MetricPoint::gauge("system.cpu", tags.clone(), 75.0));

        let dashboard = DashboardQuery::new()
            .add_query(MetricsQuery::new("http.requests"))
            .add_query(MetricsQuery::new("system.cpu"));

        let results = dashboard.execute(&state);

        assert!(results.contains_key("http.requests"));
        assert!(results.contains_key("system.cpu"));
    }
}
