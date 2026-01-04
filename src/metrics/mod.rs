//! Telemetry-Style Metrics Aggregation Module
//!
//! This module provides a metrics aggregation service that showcases
//! the unique features of this redis-rust implementation:
//!
//! - **CRDT counters** for coordination-free distributed counting
//! - **Hot key detection** for popular dashboard metrics
//! - **Pipelining** for high-throughput batch ingestion
//! - **Eventual consistency** for multi-node metric aggregation

mod types;
mod key_encoder;
mod state;
mod commands;
mod query;

pub use types::{MetricType, MetricPoint, TagSet, MetricValue};
pub use key_encoder::MetricKeyEncoder;
pub use state::{MetricsState, MetricsDelta};
pub use commands::{MetricsCommandExecutor, MetricsCommand, MetricsResult};
pub use query::{MetricsQuery, QueryResult, AggregationType, QueryExecutor};
