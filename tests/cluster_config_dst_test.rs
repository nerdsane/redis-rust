//! DST tests for Kubernetes cluster configuration
//!
//! Verifies invariants for StatefulSet peer discovery:
//! - Peer list excludes self
//! - Correct DNS format for Kubernetes headless service
//! - Correct peer count (cluster_size - 1)
//! - Replica ID parsing from pod names
//!
//! TigerStyle: These tests verify invariants that must hold for correct
//! gossip-based replication in Kubernetes.

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

/// Kubernetes cluster configuration (mirrors server_persistent.rs ClusterConfig)
/// Duplicated here to test the logic independently
struct ClusterConfig {
    replica_id: u64,
    gossip_port: u16,
    cluster_size: usize,
    peers: Vec<String>,
}

// TigerStyle: Explicit limits
const CLUSTER_SIZE_MAX: usize = 100;
const REPLICA_ID_MAX: u64 = 99;

impl ClusterConfig {
    /// Parse replica ID from StatefulSet pod name
    /// Example: "redis-rust-0" -> 0, "redis-rust-2" -> 2
    fn parse_replica_id(pod_name: &str) -> Option<u64> {
        pod_name.rsplit('-').next()?.parse().ok()
    }

    /// Build peer list from Kubernetes headless service DNS
    /// DNS format: <pod-name>.<service-name>.<namespace>.svc.cluster.local
    fn build_peer_list(
        my_replica_id: u64,
        cluster_size: usize,
        gossip_port: u16,
        statefulset_name: &str,
        service_name: &str,
        namespace: &str,
    ) -> Vec<String> {
        let mut peers = Vec::with_capacity(cluster_size.saturating_sub(1));
        for i in 0..cluster_size {
            let peer_id = i as u64;
            if peer_id != my_replica_id {
                let peer_addr = format!(
                    "{}-{}.{}.{}.svc.cluster.local:{}",
                    statefulset_name, i, service_name, namespace, gossip_port
                );
                peers.push(peer_addr);
            }
        }
        peers
    }

    /// TigerStyle: Verify all invariants hold
    fn verify_invariants(&self, statefulset_name: &str) {
        // Invariant 1: cluster_size <= max
        assert!(
            self.cluster_size <= CLUSTER_SIZE_MAX,
            "Invariant: cluster_size {} exceeds max {}",
            self.cluster_size,
            CLUSTER_SIZE_MAX
        );

        // Invariant 2: replica_id <= max
        assert!(
            self.replica_id <= REPLICA_ID_MAX,
            "Invariant: replica_id {} exceeds max {}",
            self.replica_id,
            REPLICA_ID_MAX
        );

        // Invariant 3: replica_id < cluster_size
        assert!(
            (self.replica_id as usize) < self.cluster_size,
            "Invariant: replica_id {} must be < cluster_size {}",
            self.replica_id,
            self.cluster_size
        );

        // Invariant 4: peers.len() == cluster_size - 1
        assert_eq!(
            self.peers.len(),
            self.cluster_size - 1,
            "Invariant: peers.len() {} must equal cluster_size - 1 ({})",
            self.peers.len(),
            self.cluster_size - 1
        );

        // Invariant 5: No peer contains self's pod name
        let my_pod_suffix = format!("{}-{}.", statefulset_name, self.replica_id);
        for peer in &self.peers {
            assert!(
                !peer.contains(&my_pod_suffix),
                "Invariant: peer list must not contain self: found {} in {}",
                my_pod_suffix,
                peer
            );
        }

        // Invariant 6: All peers have correct DNS format
        for peer in &self.peers {
            assert!(
                peer.contains(".svc.cluster.local:"),
                "Invariant: peer must have Kubernetes DNS format: {}",
                peer
            );
            assert!(
                peer.ends_with(&format!(":{}", self.gossip_port)),
                "Invariant: peer must end with gossip port: {}",
                peer
            );
        }

        // Invariant 7: All expected peers are present
        for i in 0..self.cluster_size {
            if i as u64 != self.replica_id {
                let expected_prefix = format!("{}-{}.", statefulset_name, i);
                assert!(
                    self.peers.iter().any(|p| p.contains(&expected_prefix)),
                    "Invariant: peer {} must be in list",
                    expected_prefix
                );
            }
        }
    }
}

/// Test replica ID parsing from various pod name formats
#[test]
fn test_replica_id_parsing() {
    // Standard StatefulSet pod names
    assert_eq!(ClusterConfig::parse_replica_id("redis-rust-0"), Some(0));
    assert_eq!(ClusterConfig::parse_replica_id("redis-rust-1"), Some(1));
    assert_eq!(ClusterConfig::parse_replica_id("redis-rust-99"), Some(99));

    // Different StatefulSet names
    assert_eq!(ClusterConfig::parse_replica_id("my-app-0"), Some(0));
    assert_eq!(
        ClusterConfig::parse_replica_id("cache-cluster-42"),
        Some(42)
    );

    // Edge cases
    assert_eq!(ClusterConfig::parse_replica_id("single-0"), Some(0));
    assert_eq!(ClusterConfig::parse_replica_id("a-b-c-d-5"), Some(5));

    // Invalid formats
    assert_eq!(ClusterConfig::parse_replica_id("no-number-suffix"), None);
    assert_eq!(ClusterConfig::parse_replica_id(""), None);
    assert_eq!(ClusterConfig::parse_replica_id("-"), None);
}

/// Test peer list generation for a 3-node cluster
#[test]
fn test_peer_list_3_node_cluster() {
    let statefulset = "redis-rust";
    let service = "redis-rust-headless";
    let namespace = "rapid-sims";
    let gossip_port = 7000u16;
    let cluster_size = 3usize;

    // Test for each replica
    for replica_id in 0..cluster_size {
        let peers = ClusterConfig::build_peer_list(
            replica_id as u64,
            cluster_size,
            gossip_port,
            statefulset,
            service,
            namespace,
        );

        let config = ClusterConfig {
            replica_id: replica_id as u64,
            gossip_port,
            cluster_size,
            peers,
        };

        // TigerStyle: Verify all invariants
        config.verify_invariants(statefulset);

        println!("Replica {}: peers = {:?}", replica_id, config.peers);
    }
}

/// Test peer list generation for a single-node cluster (edge case)
#[test]
fn test_peer_list_single_node() {
    let peers =
        ClusterConfig::build_peer_list(0, 1, 7000, "redis-rust", "redis-rust-headless", "default");

    assert!(peers.is_empty(), "Single node cluster should have no peers");
}

/// Test peer list generation for maximum cluster size
#[test]
fn test_peer_list_max_cluster() {
    let cluster_size = CLUSTER_SIZE_MAX;
    let replica_id = 50u64; // Middle of cluster

    let peers = ClusterConfig::build_peer_list(
        replica_id,
        cluster_size,
        7000,
        "redis-rust",
        "redis-rust-headless",
        "production",
    );

    let config = ClusterConfig {
        replica_id,
        gossip_port: 7000,
        cluster_size,
        peers,
    };

    config.verify_invariants("redis-rust");
    assert_eq!(config.peers.len(), CLUSTER_SIZE_MAX - 1);
}

/// DST: Test with multiple random seeds
/// Verifies invariants hold across various cluster configurations
#[test]
fn test_peer_list_dst_multi_seed() {
    const NUM_SEEDS: u64 = 50;

    for seed in 0..NUM_SEEDS {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);

        // Generate random cluster configuration
        let cluster_size = rng.gen_range(2..=20);
        let replica_id = rng.gen_range(0..cluster_size) as u64;
        let gossip_port = rng.gen_range(7000..8000);

        // Random namespace names
        let namespaces = ["default", "rapid-sims", "production", "staging", "dev"];
        let namespace = namespaces[rng.gen_range(0..namespaces.len())];

        let peers = ClusterConfig::build_peer_list(
            replica_id,
            cluster_size,
            gossip_port,
            "redis-rust",
            "redis-rust-headless",
            namespace,
        );

        let config = ClusterConfig {
            replica_id,
            gossip_port,
            cluster_size,
            peers,
        };

        // TigerStyle: Verify all invariants hold
        config.verify_invariants("redis-rust");
    }

    println!("DST passed with {} seeds", NUM_SEEDS);
}

/// DST: Test gossip message routing simulation
/// Simulates gossip messages between all peers and verifies delivery
#[test]
fn test_gossip_routing_dst() {
    const NUM_SEEDS: u64 = 20;

    for seed in 0..NUM_SEEDS {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);

        let cluster_size = rng.gen_range(3..=10);
        let gossip_port = 7000u16;

        // Build configs for all replicas
        let configs: Vec<ClusterConfig> = (0..cluster_size)
            .map(|i| {
                let peers = ClusterConfig::build_peer_list(
                    i as u64,
                    cluster_size,
                    gossip_port,
                    "redis-rust",
                    "redis-rust-headless",
                    "rapid-sims",
                );
                ClusterConfig {
                    replica_id: i as u64,
                    gossip_port,
                    cluster_size,
                    peers,
                }
            })
            .collect();

        // Verify each replica can reach all others
        for sender in &configs {
            for receiver_id in 0..cluster_size {
                if receiver_id as u64 == sender.replica_id {
                    continue; // Skip self
                }

                // Sender should have receiver in peer list
                let receiver_prefix = format!("redis-rust-{}.", receiver_id);
                let can_reach = sender.peers.iter().any(|p| p.contains(&receiver_prefix));

                assert!(
                    can_reach,
                    "Seed {}: Replica {} cannot reach replica {}. Peers: {:?}",
                    seed, sender.replica_id, receiver_id, sender.peers
                );
            }
        }

        // Verify bidirectional connectivity
        for i in 0..cluster_size {
            for j in (i + 1)..cluster_size {
                let i_has_j = configs[i]
                    .peers
                    .iter()
                    .any(|p| p.contains(&format!("redis-rust-{}.", j)));
                let j_has_i = configs[j]
                    .peers
                    .iter()
                    .any(|p| p.contains(&format!("redis-rust-{}.", i)));

                assert!(
                    i_has_j && j_has_i,
                    "Seed {}: Bidirectional connectivity failed between {} and {}",
                    seed,
                    i,
                    j
                );
            }
        }
    }

    println!("Gossip routing DST passed with {} seeds", NUM_SEEDS);
}

/// Test DNS format correctness
#[test]
fn test_dns_format() {
    let peers =
        ClusterConfig::build_peer_list(0, 3, 7000, "my-cache", "my-cache-headless", "my-namespace");

    // Verify exact DNS format
    assert!(peers
        .contains(&"my-cache-1.my-cache-headless.my-namespace.svc.cluster.local:7000".to_string()));
    assert!(peers
        .contains(&"my-cache-2.my-cache-headless.my-namespace.svc.cluster.local:7000".to_string()));
}

/// Test that peer discovery is deterministic
#[test]
fn test_peer_discovery_deterministic() {
    let config1 = ClusterConfig::build_peer_list(
        1,
        5,
        7000,
        "redis-rust",
        "redis-rust-headless",
        "rapid-sims",
    );

    let config2 = ClusterConfig::build_peer_list(
        1,
        5,
        7000,
        "redis-rust",
        "redis-rust-headless",
        "rapid-sims",
    );

    assert_eq!(config1, config2, "Peer discovery must be deterministic");
}
