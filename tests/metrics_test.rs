//! Metrics Module Integration Tests
//!
//! Tests the Telemetry-style metrics aggregation service, verifying:
//! - CRDT counter convergence
//! - Gauge LWW semantics
//! - Set unique tracking
//! - Distribution statistics
//! - Hot key detection
//! - Multi-node replication

use redis_sim::metrics::{
    MetricKeyEncoder, MetricPoint, MetricType, MetricsCommand, MetricsCommandExecutor,
    MetricsQuery, MetricsResult, MetricsState, QueryExecutor, TagSet,
};
use redis_sim::replication::lattice::ReplicaId;

// ============================================================================
// CRDT Counter Tests
// ============================================================================

#[test]
fn test_counter_basic_increment() {
    let mut state = MetricsState::new(ReplicaId::new(1));
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    state.submit(MetricPoint::counter("http.requests", tags.clone(), 100));

    let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
    assert_eq!(state.get_counter(&key), 100);
}

#[test]
fn test_counter_multiple_increments() {
    let mut state = MetricsState::new(ReplicaId::new(1));
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    for _ in 0..100 {
        state.submit(MetricPoint::counter("http.requests", tags.clone(), 1));
    }

    let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
    assert_eq!(state.get_counter(&key), 100);
}

#[test]
fn test_counter_crdt_merge_convergence() {
    // Two replicas increment same counter independently
    let mut state1 = MetricsState::new(ReplicaId::new(1));
    let mut state2 = MetricsState::new(ReplicaId::new(2));
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    // Each replica increments
    state1.submit(MetricPoint::counter("http.requests", tags.clone(), 100));
    state2.submit(MetricPoint::counter("http.requests", tags.clone(), 200));

    // Merge in both directions
    state1.merge(&state2);
    state2.merge(&state1);

    let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);

    // Both should converge to 300 (100 + 200)
    assert_eq!(state1.get_counter(&key), 300);
    assert_eq!(state2.get_counter(&key), 300);
}

#[test]
fn test_counter_crdt_idempotent_merge() {
    let mut state1 = MetricsState::new(ReplicaId::new(1));
    let state2 = MetricsState::new(ReplicaId::new(2));
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    state1.submit(MetricPoint::counter("http.requests", tags.clone(), 100));

    // Merge multiple times - should be idempotent
    state1.merge(&state2);
    state1.merge(&state2);
    state1.merge(&state2);

    let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
    assert_eq!(state1.get_counter(&key), 100);
}

#[test]
fn test_counter_delta_replication() {
    let mut state1 = MetricsState::new(ReplicaId::new(1));
    let mut state2 = MetricsState::new(ReplicaId::new(2));
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    // state1 submits counter
    state1.submit(MetricPoint::counter("http.requests", tags.clone(), 100));

    // Get and apply deltas
    let deltas = state1.drain_deltas();
    for delta in deltas {
        state2.apply_delta(delta);
    }

    let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);
    assert_eq!(state2.get_counter(&key), 100);
}

// ============================================================================
// Gauge (LWW) Tests
// ============================================================================

#[test]
fn test_gauge_last_writer_wins() {
    let mut state = MetricsState::new(ReplicaId::new(1));
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    state.submit(MetricPoint::gauge("system.cpu", tags.clone(), 50.0));
    state.submit(MetricPoint::gauge("system.cpu", tags.clone(), 75.0));
    state.submit(MetricPoint::gauge("system.cpu", tags.clone(), 60.0));

    let key = MetricKeyEncoder::encode("system.cpu", MetricType::Gauge, &tags);
    assert_eq!(state.get_gauge(&key), Some(60.0)); // Last value wins
}

#[test]
fn test_gauge_lww_merge() {
    let mut state1 = MetricsState::new(ReplicaId::new(1));
    let mut state2 = MetricsState::new(ReplicaId::new(2));
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    // state1 writes first (lower timestamp)
    state1.submit(MetricPoint::gauge("system.cpu", tags.clone(), 50.0));

    // state2 writes later (higher timestamp)
    state2.submit(MetricPoint::gauge("system.cpu", tags.clone(), 75.0));

    // Merge - state2's higher timestamp should win
    state1.merge(&state2);

    let key = MetricKeyEncoder::encode("system.cpu", MetricType::Gauge, &tags);
    assert_eq!(state1.get_gauge(&key), Some(75.0));
}

// ============================================================================
// Up-Down Counter Tests
// ============================================================================

#[test]
fn test_updown_counter_basic() {
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
    state.submit(MetricPoint::up_down_counter(
        "connections.active",
        tags.clone(),
        5,
    ));

    let key = MetricKeyEncoder::encode("connections.active", MetricType::UpDownCounter, &tags);
    assert_eq!(state.get_up_down_counter(&key), 12); // 10 - 3 + 5
}

#[test]
fn test_updown_counter_negative_result() {
    let mut state = MetricsState::new(ReplicaId::new(1));
    let tags = TagSet::from_pairs(&[("queue", "jobs")]);

    state.submit(MetricPoint::up_down_counter("queue.depth", tags.clone(), 5));
    state.submit(MetricPoint::up_down_counter("queue.depth", tags.clone(), -10));

    let key = MetricKeyEncoder::encode("queue.depth", MetricType::UpDownCounter, &tags);
    assert_eq!(state.get_up_down_counter(&key), -5);
}

// ============================================================================
// Set (ORSet) Tests
// ============================================================================

#[test]
fn test_set_unique_counting() {
    let mut state = MetricsState::new(ReplicaId::new(1));
    let tags = TagSet::from_pairs(&[("page", "/home")]);

    state.submit(MetricPoint::set("unique.users", tags.clone(), "user1"));
    state.submit(MetricPoint::set("unique.users", tags.clone(), "user2"));
    state.submit(MetricPoint::set("unique.users", tags.clone(), "user3"));
    state.submit(MetricPoint::set("unique.users", tags.clone(), "user1")); // Duplicate

    let key = MetricKeyEncoder::encode("unique.users", MetricType::Set, &tags);
    assert_eq!(state.get_set_cardinality(&key), 3); // Only 3 unique
}

#[test]
fn test_set_contains() {
    let mut state = MetricsState::new(ReplicaId::new(1));
    let tags = TagSet::from_pairs(&[("page", "/home")]);

    state.submit(MetricPoint::set("unique.users", tags.clone(), "alice"));
    state.submit(MetricPoint::set("unique.users", tags.clone(), "bob"));

    let key = MetricKeyEncoder::encode("unique.users", MetricType::Set, &tags);
    assert!(state.set_contains(&key, "alice"));
    assert!(state.set_contains(&key, "bob"));
    assert!(!state.set_contains(&key, "charlie"));
}

#[test]
fn test_set_crdt_merge() {
    let mut state1 = MetricsState::new(ReplicaId::new(1));
    let mut state2 = MetricsState::new(ReplicaId::new(2));
    let tags = TagSet::from_pairs(&[("page", "/home")]);

    // Each replica adds different users
    state1.submit(MetricPoint::set("unique.users", tags.clone(), "alice"));
    state1.submit(MetricPoint::set("unique.users", tags.clone(), "bob"));

    state2.submit(MetricPoint::set("unique.users", tags.clone(), "charlie"));
    state2.submit(MetricPoint::set("unique.users", tags.clone(), "alice")); // Same as state1

    // Merge
    state1.merge(&state2);

    let key = MetricKeyEncoder::encode("unique.users", MetricType::Set, &tags);
    assert_eq!(state1.get_set_cardinality(&key), 3); // alice, bob, charlie
}

// ============================================================================
// Distribution Tests
// ============================================================================

#[test]
fn test_distribution_statistics() {
    let mut state = MetricsState::new(ReplicaId::new(1));
    let tags = TagSet::from_pairs(&[("endpoint", "/api/users")]);

    // Add known values
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
    assert!((dist.avg() - 50.5).abs() < 0.01);
    assert!(dist.p50() >= 49.0 && dist.p50() <= 51.0);
    assert!(dist.p99() >= 98.0);
}

// ============================================================================
// Tag Set Tests
// ============================================================================

#[test]
fn test_tag_set_deterministic_hash() {
    // Same tags in different order should produce same hash
    let tags1 = TagSet::from_pairs(&[("a", "1"), ("b", "2"), ("c", "3")]);
    let tags2 = TagSet::from_pairs(&[("c", "3"), ("a", "1"), ("b", "2")]);

    assert_eq!(tags1.hash(), tags2.hash());
}

#[test]
fn test_tag_set_wildcard_matching() {
    let tags = TagSet::from_pairs(&[("host", "web01"), ("env", "prod")]);

    let pattern1 = TagSet::from_pairs(&[("host", "*")]);
    assert!(tags.matches(&pattern1));

    let pattern2 = TagSet::from_pairs(&[("host", "web01"), ("env", "*")]);
    assert!(tags.matches(&pattern2));

    let pattern3 = TagSet::from_pairs(&[("host", "web02")]);
    assert!(!tags.matches(&pattern3));
}

// ============================================================================
// Key Encoder Tests
// ============================================================================

#[test]
fn test_key_encode_decode_roundtrip() {
    let tags = TagSet::from_pairs(&[("host", "web01")]);
    let key = MetricKeyEncoder::encode("http.requests", MetricType::Counter, &tags);

    let (name, metric_type, tags_hash) = MetricKeyEncoder::decode(&key).unwrap();

    assert_eq!(name, "http.requests");
    assert_eq!(metric_type, MetricType::Counter);
    assert_eq!(tags_hash, tags.hash());
}

#[test]
fn test_key_type_detection() {
    let tags = TagSet::from_pairs(&[("host", "web01")]);

    let counter_key = MetricKeyEncoder::encode("test", MetricType::Counter, &tags);
    assert!(MetricKeyEncoder::is_metric_key(&counter_key));

    let meta_key = MetricKeyEncoder::encode_meta("test", &tags);
    assert!(MetricKeyEncoder::is_meta_key(&meta_key));
}

// ============================================================================
// Command Executor Tests
// ============================================================================

#[test]
fn test_command_executor_counter() {
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

    let result = executor.execute(
        MetricsCommand::Query {
            name: "http.requests".to_string(),
            tags: TagSet::from_pairs(&[("host", "web01")]),
        },
        0,
    );

    match result {
        MetricsResult::Integer(v) => assert_eq!(v, 100),
        _ => panic!("Expected Integer"),
    }
}

#[test]
fn test_command_executor_gauge() {
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
        _ => panic!("Expected Float"),
    }
}

#[test]
fn test_command_parsing() {
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
            assert_eq!(increment, 100);
        }
        _ => panic!("Expected Counter"),
    }
}

// ============================================================================
// Hot Key Detection Tests
// ============================================================================

#[test]
fn test_hot_key_detection_enabled() {
    let mut executor = MetricsCommandExecutor::new(1).with_hot_key_detection();

    // Submit many metrics to same key
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
            assert!(!arr.is_empty(), "Should detect hot keys");
        }
        _ => panic!("Expected Array"),
    }
}

// ============================================================================
// Query Tests
// ============================================================================

#[test]
fn test_query_wildcard() {
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

    let executor = QueryExecutor::new(&state);
    let query = MetricsQuery::new("http.*");
    let results = executor.execute(&query);

    assert_eq!(results.len(), 2);
}

// ============================================================================
// Multi-Node Simulation Tests
// ============================================================================

#[test]
fn test_multi_node_counter_convergence() {
    // Simulate 3 nodes
    let mut nodes: Vec<MetricsState> = (0..3)
        .map(|i| MetricsState::new(ReplicaId::new(i as u64)))
        .collect();

    let tags = TagSet::from_pairs(&[("service", "api")]);

    // Each node increments independently
    nodes[0].submit(MetricPoint::counter("requests", tags.clone(), 100));
    nodes[1].submit(MetricPoint::counter("requests", tags.clone(), 200));
    nodes[2].submit(MetricPoint::counter("requests", tags.clone(), 150));

    // Simulate gossip: all nodes share with all others
    let snapshots: Vec<MetricsState> = nodes.iter().cloned().collect();
    for i in 0..3 {
        for j in 0..3 {
            if i != j {
                nodes[i].merge(&snapshots[j]);
            }
        }
    }

    let key = MetricKeyEncoder::encode("requests", MetricType::Counter, &tags);

    // All nodes should converge to 450
    for (i, node) in nodes.iter().enumerate() {
        assert_eq!(
            node.get_counter(&key),
            450,
            "Node {} did not converge",
            i
        );
    }
}

#[test]
fn test_multi_node_gauge_lww() {
    let mut nodes: Vec<MetricsState> = (0..3)
        .map(|i| MetricsState::new(ReplicaId::new(i as u64)))
        .collect();

    let tags = TagSet::from_pairs(&[("host", "shared")]);

    // Each node sets gauge (last one has highest timestamp)
    nodes[0].submit(MetricPoint::gauge("cpu", tags.clone(), 50.0));
    nodes[1].submit(MetricPoint::gauge("cpu", tags.clone(), 60.0));
    nodes[2].submit(MetricPoint::gauge("cpu", tags.clone(), 70.0)); // Latest

    // Gossip
    let snapshots: Vec<MetricsState> = nodes.iter().cloned().collect();
    for i in 0..3 {
        for j in 0..3 {
            if i != j {
                nodes[i].merge(&snapshots[j]);
            }
        }
    }

    let key = MetricKeyEncoder::encode("cpu", MetricType::Gauge, &tags);

    // All nodes should have 70.0 (latest timestamp)
    for (i, node) in nodes.iter().enumerate() {
        assert_eq!(
            node.get_gauge(&key),
            Some(70.0),
            "Node {} did not get latest gauge",
            i
        );
    }
}

#[test]
fn test_high_cardinality_tags() {
    let mut state = MetricsState::new(ReplicaId::new(1));

    // Simulate high cardinality (100 hosts x 50 endpoints = 5000 unique tag combinations)
    for host in 0..100 {
        for endpoint in 0..50 {
            let tags = TagSet::from_pairs(&[
                ("host", &format!("web{:03}", host)),
                ("endpoint", &format!("/api/v1/endpoint{}", endpoint)),
            ]);
            state.submit(MetricPoint::counter("http.requests", tags, 1));
        }
    }

    // Should handle 5000 unique tag combinations
    assert_eq!(state.len(), 5000);
}

#[test]
fn test_pipelined_batch_submission() {
    let mut state = MetricsState::new(ReplicaId::new(1));

    // Simulate pipelined batch (1000 metrics)
    let metrics: Vec<MetricPoint> = (0..1000)
        .map(|i| {
            let tags = TagSet::from_pairs(&[("id", &i.to_string())]);
            MetricPoint::counter("batch.metric", tags, 1)
        })
        .collect();

    let start = std::time::Instant::now();

    for point in metrics {
        state.submit(point);
    }

    let elapsed = start.elapsed();

    // Should complete quickly (< 100ms for 1000 metrics)
    assert!(
        elapsed.as_millis() < 100,
        "Batch submission too slow: {:?}",
        elapsed
    );

    assert_eq!(state.len(), 1000);
}
