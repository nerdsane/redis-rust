//! CRDT-backed metrics state storage
//!
//! Uses the following CRDTs for different metric types:
//! - Counter: GCounter (grow-only, coordination-free increments)
//! - Gauge: LwwRegister (last-writer-wins for point-in-time values)
//! - UpDownCounter: PNCounter (supports increment and decrement)
//! - Set: ORSet (observed-remove for unique tracking)

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use crate::replication::lattice::{
    GCounter, LamportClock, LwwRegister, ORSet, PNCounter, ReplicaId,
};

use super::key_encoder::MetricKeyEncoder;
use super::types::{MetricPoint, MetricType, MetricValue, TagSet};

/// Delta representing a change to metrics state (for replication)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsDelta {
    /// The metric key
    pub key: String,
    /// Type of metric
    pub metric_type: MetricType,
    /// The delta value
    pub delta: MetricDeltaValue,
    /// Source replica
    pub source_replica: ReplicaId,
    /// Timestamp of the delta
    pub timestamp: LamportClock,
}

/// Delta value for different metric types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetricDeltaValue {
    /// Counter increment
    CounterIncrement { amount: u64 },
    /// Gauge update (full state for LWW)
    GaugeUpdate { value: f64, timestamp: LamportClock },
    /// UpDown counter change
    UpDownChange { increment: i64 },
    /// Set addition
    SetAdd { value: String },
    /// Set removal
    SetRemove { value: String },
}

/// Distribution statistics (simple implementation)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DistributionStats {
    pub count: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    /// Approximate percentiles using sorted samples
    samples: Vec<f64>,
    max_samples: usize,
}

impl DistributionStats {
    pub fn new() -> Self {
        DistributionStats {
            count: 0,
            sum: 0.0,
            min: f64::MAX,
            max: f64::MIN,
            samples: Vec::new(),
            max_samples: 1000,
        }
    }

    pub fn add(&mut self, value: f64) {
        self.count += 1;
        self.sum += value;
        self.min = self.min.min(value);
        self.max = self.max.max(value);

        // Reservoir sampling for percentiles
        if self.samples.len() < self.max_samples {
            self.samples.push(value);
            self.samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        }
    }

    pub fn avg(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }

    pub fn percentile(&self, p: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let idx = ((p / 100.0) * (self.samples.len() - 1) as f64) as usize;
        self.samples[idx.min(self.samples.len() - 1)]
    }

    pub fn p50(&self) -> f64 {
        self.percentile(50.0)
    }

    pub fn p90(&self) -> f64 {
        self.percentile(90.0)
    }

    pub fn p99(&self) -> f64 {
        self.percentile(99.0)
    }

    /// Merge two distributions
    pub fn merge(&self, other: &Self) -> Self {
        let mut merged = DistributionStats {
            count: self.count + other.count,
            sum: self.sum + other.sum,
            min: self.min.min(other.min),
            max: self.max.max(other.max),
            samples: Vec::new(),
            max_samples: self.max_samples,
        };

        // Merge samples (simple approach: combine and truncate)
        merged.samples = self.samples.clone();
        merged.samples.extend(other.samples.iter().cloned());
        merged.samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        merged.samples.truncate(merged.max_samples);

        merged
    }
}

/// CRDT-backed metrics state
#[derive(Debug, Clone)]
pub struct MetricsState {
    /// This replica's ID
    pub replica_id: ReplicaId,

    /// Lamport clock for ordering
    pub clock: LamportClock,

    /// Counter metrics (GCounter for coordination-free increments)
    counters: HashMap<String, GCounter>,

    /// Gauge metrics (LwwRegister for last-writer-wins)
    gauges: HashMap<String, LwwRegister<f64>>,

    /// Up-down counter metrics (PNCounter)
    up_down_counters: HashMap<String, PNCounter>,

    /// Set metrics (ORSet for unique tracking)
    sets: HashMap<String, ORSet<String>>,

    /// Distribution metrics
    distributions: HashMap<String, DistributionStats>,

    /// Tag metadata (key -> serialized tags)
    tag_metadata: HashMap<String, String>,

    /// Pending deltas for replication
    pending_deltas: Vec<MetricsDelta>,
}

impl MetricsState {
    /// Create new metrics state for a replica
    pub fn new(replica_id: ReplicaId) -> Self {
        MetricsState {
            replica_id,
            clock: LamportClock::new(replica_id),
            counters: HashMap::new(),
            gauges: HashMap::new(),
            up_down_counters: HashMap::new(),
            sets: HashMap::new(),
            distributions: HashMap::new(),
            tag_metadata: HashMap::new(),
            pending_deltas: Vec::new(),
        }
    }

    /// Submit a metric point
    pub fn submit(&mut self, point: MetricPoint) {
        let key = MetricKeyEncoder::encode(&point.name, point.metric_type, &point.tags);

        // Store tag metadata for later querying
        let meta_key = MetricKeyEncoder::encode_meta(&point.name, &point.tags);
        self.tag_metadata
            .entry(meta_key)
            .or_insert_with(|| point.tags.to_string());

        match point.metric_type {
            MetricType::Counter => {
                let amount = point.value.as_i64().max(0) as u64;
                self.increment_counter(&key, amount);
            }
            MetricType::Gauge => {
                let value = point.value.as_f64();
                self.set_gauge(&key, value);
            }
            MetricType::UpDownCounter => {
                let amount = point.value.as_i64();
                self.update_up_down_counter(&key, amount);
            }
            MetricType::Distribution => {
                let value = point.value.as_f64();
                self.add_distribution(&key, value);
            }
            MetricType::Set => {
                let value = point.value.as_string();
                self.add_to_set(&key, value);
            }
        }
    }

    /// Increment a counter (coordination-free)
    pub fn increment_counter(&mut self, key: &str, amount: u64) {
        let counter = self
            .counters
            .entry(key.to_string())
            .or_insert_with(GCounter::new);
        counter.increment_by(self.replica_id, amount);

        // Generate delta for replication
        self.pending_deltas.push(MetricsDelta {
            key: key.to_string(),
            metric_type: MetricType::Counter,
            delta: MetricDeltaValue::CounterIncrement { amount },
            source_replica: self.replica_id,
            timestamp: self.clock.tick(),
        });
    }

    /// Get counter value
    pub fn get_counter(&self, key: &str) -> u64 {
        self.counters.get(key).map(|c| c.value()).unwrap_or(0)
    }

    /// Set a gauge value (last-writer-wins)
    pub fn set_gauge(&mut self, key: &str, value: f64) {
        let timestamp = self.clock.tick();
        let gauge = self
            .gauges
            .entry(key.to_string())
            .or_insert_with(|| LwwRegister::new(self.replica_id));
        gauge.value = Some(value);
        gauge.timestamp = timestamp;

        // Generate delta for replication
        self.pending_deltas.push(MetricsDelta {
            key: key.to_string(),
            metric_type: MetricType::Gauge,
            delta: MetricDeltaValue::GaugeUpdate { value, timestamp },
            source_replica: self.replica_id,
            timestamp,
        });
    }

    /// Get gauge value
    pub fn get_gauge(&self, key: &str) -> Option<f64> {
        self.gauges.get(key).and_then(|g| g.value)
    }

    /// Update up-down counter
    pub fn update_up_down_counter(&mut self, key: &str, amount: i64) {
        let counter = self
            .up_down_counters
            .entry(key.to_string())
            .or_insert_with(PNCounter::new);

        if amount >= 0 {
            counter.increment_by(self.replica_id, amount as u64);
        } else {
            counter.decrement_by(self.replica_id, (-amount) as u64);
        }

        // Generate delta for replication
        self.pending_deltas.push(MetricsDelta {
            key: key.to_string(),
            metric_type: MetricType::UpDownCounter,
            delta: MetricDeltaValue::UpDownChange { increment: amount },
            source_replica: self.replica_id,
            timestamp: self.clock.tick(),
        });
    }

    /// Get up-down counter value
    pub fn get_up_down_counter(&self, key: &str) -> i64 {
        self.up_down_counters
            .get(key)
            .map(|c| c.value())
            .unwrap_or(0)
    }

    /// Add to distribution (for histograms/percentiles)
    pub fn add_distribution(&mut self, key: &str, value: f64) {
        let dist = self
            .distributions
            .entry(key.to_string())
            .or_insert_with(DistributionStats::new);
        dist.add(value);
    }

    /// Get distribution statistics
    pub fn get_distribution(&self, key: &str) -> Option<&DistributionStats> {
        self.distributions.get(key)
    }

    /// Add value to set (for unique tracking)
    pub fn add_to_set(&mut self, key: &str, value: String) {
        let set = self
            .sets
            .entry(key.to_string())
            .or_insert_with(ORSet::new);
        set.add(value.clone(), self.replica_id);

        // Generate delta for replication
        self.pending_deltas.push(MetricsDelta {
            key: key.to_string(),
            metric_type: MetricType::Set,
            delta: MetricDeltaValue::SetAdd { value },
            source_replica: self.replica_id,
            timestamp: self.clock.tick(),
        });
    }

    /// Get set cardinality (unique count)
    pub fn get_set_cardinality(&self, key: &str) -> usize {
        self.sets.get(key).map(|s| s.len()).unwrap_or(0)
    }

    /// Check if value exists in set
    pub fn set_contains(&self, key: &str, value: &str) -> bool {
        self.sets
            .get(key)
            .map(|s| s.contains(&value.to_string()))
            .unwrap_or(false)
    }

    /// Drain pending deltas for replication
    pub fn drain_deltas(&mut self) -> Vec<MetricsDelta> {
        std::mem::take(&mut self.pending_deltas)
    }

    /// Apply a delta from another replica
    pub fn apply_delta(&mut self, delta: MetricsDelta) {
        // Update our clock
        self.clock.update(&delta.timestamp);

        match delta.delta {
            MetricDeltaValue::CounterIncrement { amount } => {
                let counter = self
                    .counters
                    .entry(delta.key)
                    .or_insert_with(GCounter::new);
                counter.increment_by(delta.source_replica, amount);
            }
            MetricDeltaValue::GaugeUpdate { value, timestamp } => {
                let gauge = self
                    .gauges
                    .entry(delta.key)
                    .or_insert_with(|| LwwRegister::new(delta.source_replica));
                // Only update if this is newer (LWW semantics)
                if timestamp > gauge.timestamp {
                    gauge.value = Some(value);
                    gauge.timestamp = timestamp;
                }
            }
            MetricDeltaValue::UpDownChange { increment } => {
                let counter = self
                    .up_down_counters
                    .entry(delta.key)
                    .or_insert_with(PNCounter::new);
                if increment >= 0 {
                    counter.increment_by(delta.source_replica, increment as u64);
                } else {
                    counter.decrement_by(delta.source_replica, (-increment) as u64);
                }
            }
            MetricDeltaValue::SetAdd { value } => {
                let set = self.sets.entry(delta.key).or_insert_with(ORSet::new);
                set.add(value, delta.source_replica);
            }
            MetricDeltaValue::SetRemove { value } => {
                if let Some(set) = self.sets.get_mut(&delta.key) {
                    set.remove(&value);
                }
            }
        }
    }

    /// Merge with another MetricsState (for full state sync)
    pub fn merge(&mut self, other: &MetricsState) {
        // Merge counters
        for (key, other_counter) in &other.counters {
            let counter = self
                .counters
                .entry(key.clone())
                .or_insert_with(GCounter::new);
            *counter = counter.merge(other_counter);
        }

        // Merge gauges (LWW)
        for (key, other_gauge) in &other.gauges {
            let gauge = self
                .gauges
                .entry(key.clone())
                .or_insert_with(|| LwwRegister::new(other.replica_id));
            *gauge = gauge.merge(other_gauge);
        }

        // Merge up-down counters
        for (key, other_counter) in &other.up_down_counters {
            let counter = self
                .up_down_counters
                .entry(key.clone())
                .or_insert_with(PNCounter::new);
            *counter = counter.merge(other_counter);
        }

        // Merge sets
        for (key, other_set) in &other.sets {
            let set = self.sets.entry(key.clone()).or_insert_with(ORSet::new);
            *set = set.merge(other_set);
        }

        // Merge distributions
        for (key, other_dist) in &other.distributions {
            let dist = self
                .distributions
                .entry(key.clone())
                .or_insert_with(DistributionStats::new);
            *dist = dist.merge(other_dist);
        }

        // Merge tag metadata
        for (key, tags) in &other.tag_metadata {
            self.tag_metadata.entry(key.clone()).or_insert(tags.clone());
        }
    }

    /// Get all metric keys
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.counters.keys().cloned().collect();
        keys.extend(self.gauges.keys().cloned());
        keys.extend(self.up_down_counters.keys().cloned());
        keys.extend(self.sets.keys().cloned());
        keys.extend(self.distributions.keys().cloned());
        keys.sort();
        keys.dedup();
        keys
    }

    /// Get metrics count
    pub fn len(&self) -> usize {
        self.counters.len()
            + self.gauges.len()
            + self.up_down_counters.len()
            + self.sets.len()
            + self.distributions.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_increment() {
        let mut state = MetricsState::new(ReplicaId::new(1));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        state.submit(MetricPoint::counter("http.requests", tags.clone(), 100));
        state.submit(MetricPoint::counter("http.requests", tags.clone(), 50));

        let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
        assert_eq!(state.get_counter(&key), 150);
    }

    #[test]
    fn test_gauge_lww() {
        let mut state = MetricsState::new(ReplicaId::new(1));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        state.submit(MetricPoint::gauge("system.cpu", tags.clone(), 50.0));
        state.submit(MetricPoint::gauge("system.cpu", tags.clone(), 75.0));

        let key = MetricKeyEncoder::encode("system.cpu", MetricType::Gauge, &tags);
        assert_eq!(state.get_gauge(&key), Some(75.0));
    }

    #[test]
    fn test_up_down_counter() {
        let mut state = MetricsState::new(ReplicaId::new(1));
        let tags = TagSet::from_pairs(&[("pool", "main")]);

        state.submit(MetricPoint::up_down_counter(
            "connections.active",
            tags.clone(),
            10,
        ));
        state.submit(MetricPoint::up_down_counter(
            "connections.active",
            tags.clone(),
            -3,
        ));

        let key = MetricKeyEncoder::encode("connections.active", MetricType::UpDownCounter, &tags);
        assert_eq!(state.get_up_down_counter(&key), 7);
    }

    #[test]
    fn test_set_unique_tracking() {
        let mut state = MetricsState::new(ReplicaId::new(1));
        let tags = TagSet::from_pairs(&[("page", "/home")]);

        state.submit(MetricPoint::set("unique.users", tags.clone(), "user1"));
        state.submit(MetricPoint::set("unique.users", tags.clone(), "user2"));
        state.submit(MetricPoint::set("unique.users", tags.clone(), "user1")); // Duplicate

        let key = MetricKeyEncoder::encode("unique.users", MetricType::Set, &tags);
        assert_eq!(state.get_set_cardinality(&key), 2);
        assert!(state.set_contains(&key, "user1"));
        assert!(state.set_contains(&key, "user2"));
        assert!(!state.set_contains(&key, "user3"));
    }

    #[test]
    fn test_distribution_stats() {
        let mut state = MetricsState::new(ReplicaId::new(1));
        let tags = TagSet::from_pairs(&[("endpoint", "/api")]);

        for i in 1..=100 {
            state.submit(MetricPoint::distribution(
                "http.latency",
                tags.clone(),
                i as f64,
            ));
        }

        let key = MetricKeyEncoder::encode("http.latency", MetricType::Distribution, &tags);
        let dist = state.get_distribution(&key).unwrap();

        assert_eq!(dist.count, 100);
        assert_eq!(dist.min, 1.0);
        assert_eq!(dist.max, 100.0);
        assert_eq!(dist.avg(), 50.5);
        assert!(dist.p50() >= 49.0 && dist.p50() <= 51.0);
        assert!(dist.p99() >= 98.0 && dist.p99() <= 100.0);
    }

    #[test]
    fn test_counter_crdt_merge() {
        let mut state1 = MetricsState::new(ReplicaId::new(1));
        let mut state2 = MetricsState::new(ReplicaId::new(2));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        // Each replica increments independently
        state1.submit(MetricPoint::counter("http.requests", tags.clone(), 100));
        state2.submit(MetricPoint::counter("http.requests", tags.clone(), 150));

        // Merge state2 into state1
        state1.merge(&state2);

        let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
        // Both contributions should be counted
        assert_eq!(state1.get_counter(&key), 250);
    }

    #[test]
    fn test_gauge_crdt_merge_lww() {
        let mut state1 = MetricsState::new(ReplicaId::new(1));
        let mut state2 = MetricsState::new(ReplicaId::new(2));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        // state1 writes first
        state1.submit(MetricPoint::gauge("system.cpu", tags.clone(), 50.0));

        // state2 writes later (higher timestamp)
        state2.submit(MetricPoint::gauge("system.cpu", tags.clone(), 75.0));

        // Merge - state2's value should win (higher timestamp)
        state1.merge(&state2);

        let key = MetricKeyEncoder::encode("system.cpu", MetricType::Gauge, &tags);
        assert_eq!(state1.get_gauge(&key), Some(75.0));
    }

    #[test]
    fn test_delta_replication() {
        let mut state1 = MetricsState::new(ReplicaId::new(1));
        let mut state2 = MetricsState::new(ReplicaId::new(2));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        // state1 submits metric
        state1.submit(MetricPoint::counter("http.requests", tags.clone(), 100));

        // Get deltas from state1
        let deltas = state1.drain_deltas();
        assert_eq!(deltas.len(), 1);

        // Apply deltas to state2
        for delta in deltas {
            state2.apply_delta(delta);
        }

        let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
        assert_eq!(state2.get_counter(&key), 100);
    }

    #[test]
    fn test_bidirectional_delta_replication() {
        let mut state1 = MetricsState::new(ReplicaId::new(1));
        let mut state2 = MetricsState::new(ReplicaId::new(2));
        let tags = TagSet::from_pairs(&[("host", "web01")]);

        // Both replicas submit independently
        state1.submit(MetricPoint::counter("http.requests", tags.clone(), 100));
        state2.submit(MetricPoint::counter("http.requests", tags.clone(), 200));

        // Exchange deltas
        let deltas1 = state1.drain_deltas();
        let deltas2 = state2.drain_deltas();

        for delta in deltas1 {
            state2.apply_delta(delta);
        }
        for delta in deltas2 {
            state1.apply_delta(delta);
        }

        // Both should converge to same value
        let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
        assert_eq!(state1.get_counter(&key), 300);
        assert_eq!(state2.get_counter(&key), 300);
    }
}
