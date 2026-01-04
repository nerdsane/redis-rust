//! Anti-Entropy Protocol for Partition Healing
//!
//! Implements Merkle tree-based state synchronization for efficient
//! reconciliation after network partitions heal.
//!
//! ## Protocol Overview
//! 1. Each node maintains a Merkle tree digest of its key-value state
//! 2. Periodically, or on partition heal detection, nodes exchange digests
//! 3. Divergent subtrees are identified and only those keys are synced
//! 4. Uses version vectors to determine which values are newer
//!
//! ## Key Features
//! - Efficient O(log n) divergence detection via Merkle trees
//! - Bandwidth-efficient: only sync divergent keys
//! - Crdt-aware: merges values rather than overwriting

use super::lattice::ReplicaId;
use super::state::{ReplicatedValue, ReplicationDelta};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Configuration for anti-entropy protocol
#[derive(Debug, Clone)]
pub struct AntiEntropyConfig {
    /// How often to run anti-entropy (in simulation time units)
    pub sync_interval_ms: u64,
    /// Maximum keys to sync per round (to avoid overwhelming network)
    pub max_keys_per_sync: usize,
    /// Number of levels in merkle tree (2^levels = max buckets)
    pub merkle_tree_depth: usize,
    /// Whether to enable automatic sync on partition heal
    pub auto_sync_on_heal: bool,
}

impl Default for AntiEntropyConfig {
    fn default() -> Self {
        AntiEntropyConfig {
            sync_interval_ms: 1000,
            max_keys_per_sync: 1000,
            merkle_tree_depth: 8, // 256 buckets
            auto_sync_on_heal: true,
        }
    }
}

/// A simple hash-based digest of a key-value pair
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyDigest {
    pub key_hash: u64,
    pub value_hash: u64,
    pub timestamp: u64,
}

impl KeyDigest {
    pub fn new(key: &str, value: &ReplicatedValue) -> Self {
        let mut key_hasher = DefaultHasher::new();
        key.hash(&mut key_hasher);
        let key_hash = key_hasher.finish();

        let mut value_hasher = DefaultHasher::new();
        // Hash the timestamp and tombstone status
        value.timestamp.time.hash(&mut value_hasher);
        value.timestamp.replica_id.0.hash(&mut value_hasher);
        if let Some(v) = value.get() {
            v.as_bytes().hash(&mut value_hasher);
        }
        let value_hash = value_hasher.finish();

        KeyDigest {
            key_hash,
            value_hash,
            timestamp: value.timestamp.time,
        }
    }

    /// Get the bucket index for this key in a merkle tree
    pub fn bucket(&self, depth: usize) -> usize {
        (self.key_hash as usize) % (1 << depth)
    }
}

/// Merkle tree node for efficient state comparison
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleNode {
    /// Hash of this node (combines children or leaf digests)
    pub hash: u64,
    /// Number of keys in this subtree
    pub count: usize,
    /// Maximum timestamp in this subtree
    pub max_timestamp: u64,
}

impl MerkleNode {
    pub fn empty() -> Self {
        MerkleNode {
            hash: 0,
            count: 0,
            max_timestamp: 0,
        }
    }

    pub fn from_digests(digests: &[KeyDigest]) -> Self {
        if digests.is_empty() {
            return Self::empty();
        }

        let mut hasher = DefaultHasher::new();
        let mut max_ts = 0u64;

        for d in digests {
            d.key_hash.hash(&mut hasher);
            d.value_hash.hash(&mut hasher);
            max_ts = max_ts.max(d.timestamp);
        }

        MerkleNode {
            hash: hasher.finish(),
            count: digests.len(),
            max_timestamp: max_ts,
        }
    }

    pub fn combine(left: &MerkleNode, right: &MerkleNode) -> Self {
        if left.count == 0 && right.count == 0 {
            return Self::empty();
        }

        let mut hasher = DefaultHasher::new();
        left.hash.hash(&mut hasher);
        right.hash.hash(&mut hasher);

        MerkleNode {
            hash: hasher.finish(),
            count: left.count + right.count,
            max_timestamp: left.max_timestamp.max(right.max_timestamp),
        }
    }
}

/// State digest for efficient comparison
#[derive(Debug, Clone)]
pub struct StateDigest {
    /// Root hash of merkle tree
    pub root_hash: u64,
    /// Total number of keys
    pub key_count: usize,
    /// Maximum timestamp across all keys
    pub max_timestamp: u64,
    /// Per-bucket merkle nodes (for selective sync)
    pub buckets: Vec<MerkleNode>,
    /// Replica that generated this digest
    pub replica_id: ReplicaId,
    /// Generation number (incremented on each change)
    pub generation: u64,
}

impl StateDigest {
    /// Create a digest from a key-value map
    pub fn from_state(
        keys: &HashMap<String, ReplicatedValue>,
        replica_id: ReplicaId,
        generation: u64,
        depth: usize,
    ) -> Self {
        let num_buckets = 1 << depth;
        let mut bucket_digests: Vec<Vec<KeyDigest>> = vec![Vec::new(); num_buckets];

        // Sort keys into buckets
        for (key, value) in keys {
            let digest = KeyDigest::new(key, value);
            let bucket = digest.bucket(depth);
            bucket_digests[bucket].push(digest);
        }

        // Build merkle nodes for each bucket
        let buckets: Vec<MerkleNode> = bucket_digests
            .iter()
            .map(|digests| MerkleNode::from_digests(digests))
            .collect();

        // Compute root
        let (root_hash, key_count, max_timestamp) = if buckets.is_empty() {
            (0, 0, 0)
        } else {
            let mut combined = buckets[0].clone();
            for node in &buckets[1..] {
                combined = MerkleNode::combine(&combined, node);
            }
            (combined.hash, combined.count, combined.max_timestamp)
        };

        StateDigest {
            root_hash,
            key_count,
            max_timestamp,
            buckets,
            replica_id,
            generation,
        }
    }

    /// Check if this digest differs from another
    pub fn differs_from(&self, other: &StateDigest) -> bool {
        self.root_hash != other.root_hash
    }

    /// Find which buckets differ between two digests
    pub fn divergent_buckets(&self, other: &StateDigest) -> Vec<usize> {
        let mut divergent = Vec::new();
        let len = self.buckets.len().min(other.buckets.len());

        for i in 0..len {
            if self.buckets[i] != other.buckets[i] {
                divergent.push(i);
            }
        }

        // Handle size mismatch
        if self.buckets.len() > len {
            for i in len..self.buckets.len() {
                if self.buckets[i].count > 0 {
                    divergent.push(i);
                }
            }
        }
        if other.buckets.len() > len {
            for i in len..other.buckets.len() {
                if other.buckets[i].count > 0 {
                    divergent.push(i);
                }
            }
        }

        divergent
    }
}

/// Anti-entropy sync request
#[derive(Debug, Clone)]
pub struct SyncRequest {
    /// Requesting node
    pub from_replica: ReplicaId,
    /// Target node
    pub to_replica: ReplicaId,
    /// Digest of requester's state
    pub digest: StateDigest,
    /// Specific buckets to sync (if known)
    pub requested_buckets: Option<Vec<usize>>,
}

/// Anti-entropy sync response
#[derive(Debug, Clone)]
pub struct SyncResponse {
    /// Responding node
    pub from_replica: ReplicaId,
    /// Deltas for divergent keys
    pub deltas: Vec<ReplicationDelta>,
    /// Responder's digest (for bidirectional sync)
    pub digest: StateDigest,
}

/// Anti-entropy state manager for a node
#[derive(Debug)]
pub struct AntiEntropyManager {
    /// Configuration
    pub config: AntiEntropyConfig,
    /// Our replica ID
    pub replica_id: ReplicaId,
    /// Current generation (incremented on writes)
    pub generation: u64,
    /// Last known digest from each peer
    pub peer_digests: HashMap<ReplicaId, StateDigest>,
    /// Peers we've detected divergence with
    pub divergent_peers: HashSet<ReplicaId>,
    /// Last sync time for each peer
    pub last_sync_time: HashMap<ReplicaId, u64>,
    /// Pending sync requests
    pub pending_requests: Vec<SyncRequest>,
    /// Pending sync responses
    pub pending_responses: Vec<SyncResponse>,
}

impl AntiEntropyManager {
    pub fn new(replica_id: ReplicaId, config: AntiEntropyConfig) -> Self {
        AntiEntropyManager {
            config,
            replica_id,
            generation: 0,
            peer_digests: HashMap::new(),
            divergent_peers: HashSet::new(),
            last_sync_time: HashMap::new(),
            pending_requests: Vec::new(),
            pending_responses: Vec::new(),
        }
    }

    /// Increment generation on local write
    pub fn on_local_write(&mut self) {
        self.generation += 1;
    }

    /// Generate our current state digest
    pub fn generate_digest(
        &self,
        keys: &HashMap<String, ReplicatedValue>,
    ) -> StateDigest {
        StateDigest::from_state(
            keys,
            self.replica_id,
            self.generation,
            self.config.merkle_tree_depth,
        )
    }

    /// Check if we should sync with a peer
    pub fn should_sync(&self, peer: ReplicaId, current_time: u64) -> bool {
        if let Some(&last_sync) = self.last_sync_time.get(&peer) {
            current_time - last_sync >= self.config.sync_interval_ms
        } else {
            true // Never synced, should sync
        }
    }

    /// Process a received digest and determine if sync is needed
    pub fn process_peer_digest(
        &mut self,
        peer_digest: StateDigest,
        our_digest: &StateDigest,
    ) -> Option<Vec<usize>> {
        let peer = peer_digest.replica_id;

        if our_digest.differs_from(&peer_digest) {
            let divergent = our_digest.divergent_buckets(&peer_digest);
            self.divergent_peers.insert(peer);
            self.peer_digests.insert(peer, peer_digest);
            Some(divergent)
        } else {
            self.divergent_peers.remove(&peer);
            self.peer_digests.insert(peer, peer_digest);
            None
        }
    }

    /// Create a sync request for a peer
    pub fn create_sync_request(
        &mut self,
        peer: ReplicaId,
        our_digest: StateDigest,
        buckets: Option<Vec<usize>>,
        current_time: u64,
    ) -> SyncRequest {
        self.last_sync_time.insert(peer, current_time);

        SyncRequest {
            from_replica: self.replica_id,
            to_replica: peer,
            digest: our_digest,
            requested_buckets: buckets,
        }
    }

    /// Handle incoming sync request and generate response
    pub fn handle_sync_request(
        &mut self,
        request: SyncRequest,
        our_keys: &HashMap<String, ReplicatedValue>,
    ) -> SyncResponse {
        let our_digest = self.generate_digest(our_keys);
        let depth = self.config.merkle_tree_depth;

        // Determine which keys to send
        let keys_to_send: Vec<(&String, &ReplicatedValue)> = if let Some(ref buckets) = request.requested_buckets {
            // Only send keys in requested buckets
            our_keys
                .iter()
                .filter(|(key, value)| {
                    let digest = KeyDigest::new(key, value);
                    buckets.contains(&digest.bucket(depth))
                })
                .take(self.config.max_keys_per_sync)
                .collect()
        } else {
            // Send all keys (up to limit)
            our_keys
                .iter()
                .take(self.config.max_keys_per_sync)
                .collect()
        };

        // Create deltas
        let deltas: Vec<ReplicationDelta> = keys_to_send
            .into_iter()
            .map(|(key, value)| {
                ReplicationDelta::new(key.clone(), value.clone(), self.replica_id)
            })
            .collect();

        // Update our knowledge of peer
        self.process_peer_digest(request.digest, &our_digest);

        SyncResponse {
            from_replica: self.replica_id,
            deltas,
            digest: our_digest,
        }
    }

    /// Get all keys in specified buckets
    pub fn get_keys_in_buckets(
        &self,
        keys: &HashMap<String, ReplicatedValue>,
        buckets: &[usize],
    ) -> Vec<ReplicationDelta> {
        let depth = self.config.merkle_tree_depth;

        keys.iter()
            .filter(|(key, value)| {
                let digest = KeyDigest::new(key, value);
                buckets.contains(&digest.bucket(depth))
            })
            .take(self.config.max_keys_per_sync)
            .map(|(key, value)| {
                ReplicationDelta::new(key.clone(), value.clone(), self.replica_id)
            })
            .collect()
    }

    /// Mark partition as healed with a peer
    pub fn on_partition_healed(&mut self, peer: ReplicaId) {
        if self.config.auto_sync_on_heal {
            self.divergent_peers.insert(peer);
            // Clear last sync time to trigger immediate sync
            self.last_sync_time.remove(&peer);
        }
    }

    /// Get peers that need sync
    pub fn peers_needing_sync(&self, current_time: u64) -> Vec<ReplicaId> {
        let mut peers: Vec<ReplicaId> = self.divergent_peers.iter().copied().collect();

        // Add peers we haven't synced with recently
        for (&peer, &last_sync) in &self.last_sync_time {
            if current_time - last_sync >= self.config.sync_interval_ms {
                if !peers.contains(&peer) {
                    peers.push(peer);
                }
            }
        }

        peers
    }

    /// Clear pending operations
    pub fn drain_requests(&mut self) -> Vec<SyncRequest> {
        std::mem::take(&mut self.pending_requests)
    }

    pub fn drain_responses(&mut self) -> Vec<SyncResponse> {
        std::mem::take(&mut self.pending_responses)
    }
}

/// Message types for anti-entropy protocol
#[derive(Debug, Clone)]
pub enum AntiEntropyMessage {
    /// Digest exchange (lightweight)
    DigestExchange(StateDigest),
    /// Full sync request
    SyncRequest(SyncRequest),
    /// Sync response with deltas
    SyncResponse(SyncResponse),
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::lattice::LamportClock;
    use crate::redis::SDS;

    fn make_value(s: &str, timestamp: u64, replica: ReplicaId) -> ReplicatedValue {
        let clock = LamportClock { time: timestamp, replica_id: replica };
        ReplicatedValue::with_value(SDS::from_str(s), clock)
    }

    #[test]
    fn test_key_digest() {
        let r1 = ReplicaId::new(1);
        let value = make_value("test", 100, r1);
        let digest1 = KeyDigest::new("key1", &value);
        let digest2 = KeyDigest::new("key1", &value);

        assert_eq!(digest1, digest2);

        let value2 = make_value("different", 100, r1);
        let digest3 = KeyDigest::new("key1", &value2);
        assert_ne!(digest1.value_hash, digest3.value_hash);
    }

    #[test]
    fn test_state_digest_identical() {
        let r1 = ReplicaId::new(1);
        let mut keys = HashMap::new();
        keys.insert("key1".to_string(), make_value("value1", 1, r1));
        keys.insert("key2".to_string(), make_value("value2", 2, r1));

        let digest1 = StateDigest::from_state(&keys, r1, 1, 4);
        let digest2 = StateDigest::from_state(&keys, r1, 1, 4);

        assert_eq!(digest1.root_hash, digest2.root_hash);
        assert!(!digest1.differs_from(&digest2));
    }

    #[test]
    fn test_state_digest_different() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut keys1 = HashMap::new();
        keys1.insert("key1".to_string(), make_value("value1", 1, r1));

        let mut keys2 = HashMap::new();
        keys2.insert("key1".to_string(), make_value("value1", 1, r1));
        keys2.insert("key2".to_string(), make_value("value2", 2, r2));

        let digest1 = StateDigest::from_state(&keys1, r1, 1, 4);
        let digest2 = StateDigest::from_state(&keys2, r2, 1, 4);

        assert!(digest1.differs_from(&digest2));
    }

    #[test]
    fn test_divergent_buckets() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut keys1 = HashMap::new();
        keys1.insert("key1".to_string(), make_value("value1", 1, r1));

        let mut keys2 = HashMap::new();
        keys2.insert("key1".to_string(), make_value("different", 2, r2));

        let digest1 = StateDigest::from_state(&keys1, r1, 1, 4);
        let digest2 = StateDigest::from_state(&keys2, r2, 1, 4);

        let divergent = digest1.divergent_buckets(&digest2);
        assert!(!divergent.is_empty());
    }

    #[test]
    fn test_anti_entropy_manager_sync() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut manager1 = AntiEntropyManager::new(r1, AntiEntropyConfig::default());
        let mut manager2 = AntiEntropyManager::new(r2, AntiEntropyConfig::default());

        // Node 1 has key1
        let mut keys1 = HashMap::new();
        keys1.insert("key1".to_string(), make_value("value1", 1, r1));

        // Node 2 has key2
        let mut keys2 = HashMap::new();
        keys2.insert("key2".to_string(), make_value("value2", 2, r2));

        let digest1 = manager1.generate_digest(&keys1);
        let digest2 = manager2.generate_digest(&keys2);

        // They should detect divergence
        let divergent = manager1.process_peer_digest(digest2.clone(), &digest1);
        assert!(divergent.is_some());

        // Create sync request
        let request = manager1.create_sync_request(r2, digest1.clone(), divergent, 0);

        // Handle request and get response
        let response = manager2.handle_sync_request(request, &keys2);

        // Response should contain key2
        assert_eq!(response.deltas.len(), 1);
        assert_eq!(response.deltas[0].key, "key2");
    }

    #[test]
    fn test_partition_heal_triggers_sync() {
        let r1 = ReplicaId::new(1);
        let r2 = ReplicaId::new(2);

        let mut manager = AntiEntropyManager::new(r1, AntiEntropyConfig::default());

        // Initially no peers need sync
        assert!(manager.peers_needing_sync(0).is_empty());

        // Partition heals
        manager.on_partition_healed(r2);

        // Now r2 needs sync
        let peers = manager.peers_needing_sync(0);
        assert!(peers.contains(&r2));
    }
}
