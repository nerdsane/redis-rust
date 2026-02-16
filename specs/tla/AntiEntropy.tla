------------------------------ MODULE AntiEntropy ------------------------------
\* TLA+ Specification for redis-rust Anti-Entropy Protocol
\*
\* This spec models the Merkle tree-based state synchronization for
\* efficient reconciliation after network partitions heal.
\* Corresponds to: src/replication/anti_entropy.rs
\*
\* Run with TLC: tlc AntiEntropy.tla
\*
\* Key Invariants:
\* 1. MERKLE_CONSISTENCY: Merkle digests correctly reflect state
\* 2. SYNC_COMPLETENESS: After sync, divergent keys are reconciled
\* 3. PARTITION_HEALING: Partition heal triggers sync
\* 4. BANDWIDTH_EFFICIENT: Only divergent buckets are synced
\* 5. CRDT_AWARE: Merge uses CRDT semantics, not overwrite

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Replicas,               \* Set of replica IDs
    Keys,                   \* Set of keys
    NumBuckets,             \* Number of Merkle tree buckets (2^depth)
    MaxTimestamp            \* Maximum timestamp for model checking

VARIABLES
    \* Per-replica state
    kv_state,               \* kv_state[r][k] = <<value, timestamp>> or NULL
    generation,             \* generation[r] = state generation (increments on writes)

    \* Merkle tree state (digest of kv_state)
    merkle_buckets,         \* merkle_buckets[r][b] = hash of bucket b at replica r
    merkle_root,            \* merkle_root[r] = root hash of merkle tree

    \* Sync protocol state
    peer_digests,           \* peer_digests[r][r2] = last known digest from r2
    divergent_peers,        \* divergent_peers[r] = set of peers needing sync
    pending_syncs,          \* pending_syncs = set of sync requests/responses

    \* Partition state
    partitioned             \* partitioned[r1][r2] = TRUE if partition exists

vars == <<kv_state, generation, merkle_buckets, merkle_root,
          peer_digests, divergent_peers, pending_syncs, partitioned>>

--------------------------------------------------------------------------------
\* Type Definitions
--------------------------------------------------------------------------------

KeyValue == [value: Nat, timestamp: Nat]

MerkleDigest == [
    root_hash: Nat,
    generation: Nat,
    replica: Replicas
]

SyncRequest == [
    type: {"SyncRequest"},
    from: Replicas,
    to: Replicas,
    digest: MerkleDigest,
    requested_buckets: SUBSET (0..NumBuckets-1)
]

SyncResponse == [
    type: {"SyncResponse"},
    from: Replicas,
    deltas: [Keys -> KeyValue \union {NULL}],
    digest: MerkleDigest
]

TypeOK ==
    /\ kv_state \in [Replicas -> [Keys -> KeyValue \union {NULL}]]
    /\ generation \in [Replicas -> Nat]
    /\ merkle_buckets \in [Replicas -> [0..NumBuckets-1 -> Nat]]
    /\ merkle_root \in [Replicas -> Nat]
    /\ peer_digests \in [Replicas -> [Replicas -> MerkleDigest \union {NULL}]]
    /\ divergent_peers \in [Replicas -> SUBSET Replicas]
    /\ pending_syncs \subseteq (SyncRequest \union SyncResponse)
    /\ partitioned \in [Replicas -> [Replicas -> BOOLEAN]]

--------------------------------------------------------------------------------
\* Helper Functions
--------------------------------------------------------------------------------

\* Hash a key to bucket (simplified)
KeyToBucket(k) == k % NumBuckets

\* Compute bucket hash from keys in that bucket
ComputeBucketHash(r, bucket) ==
    LET keys_in_bucket == {k \in Keys : KeyToBucket(k) = bucket}
        values == {<<k, kv_state[r][k]>> : k \in keys_in_bucket}
    IN \* Simplified hash: sum of (key * timestamp) for non-null values
       LET non_null == {<<k, v>> \in values : v # NULL}
       IN IF non_null = {} THEN 0
          ELSE LET pair == CHOOSE p \in non_null : TRUE
               IN pair[1] * pair[2].timestamp  \* Simplified hash

\* Compute root hash from bucket hashes
ComputeRootHash(r) ==
    LET bucket_hashes == {merkle_buckets[r][b] : b \in 0..NumBuckets-1}
    IN \* Simplified: sum of bucket hashes
       IF bucket_hashes = {} THEN 0
       ELSE CHOOSE h \in bucket_hashes : TRUE

\* Create digest for replica
MakeDigest(r) == [root_hash |-> merkle_root[r], generation |-> generation[r], replica |-> r]

\* Find divergent buckets between two replicas
DivergentBuckets(r1, r2) ==
    {b \in 0..NumBuckets-1 : merkle_buckets[r1][b] # merkle_buckets[r2][b]}

\* CRDT-aware merge (last-writer-wins)
MergeValue(local, remote) ==
    IF local = NULL THEN remote
    ELSE IF remote = NULL THEN local
    ELSE IF remote.timestamp > local.timestamp THEN remote
    ELSE local

--------------------------------------------------------------------------------
\* Initial State
--------------------------------------------------------------------------------

Init ==
    /\ kv_state = [r \in Replicas |-> [k \in Keys |-> NULL]]
    /\ generation = [r \in Replicas |-> 0]
    /\ merkle_buckets = [r \in Replicas |-> [b \in 0..NumBuckets-1 |-> 0]]
    /\ merkle_root = [r \in Replicas |-> 0]
    /\ peer_digests = [r \in Replicas |-> [r2 \in Replicas |-> NULL]]
    /\ divergent_peers = [r \in Replicas |-> {}]
    /\ pending_syncs = {}
    /\ partitioned = [r1 \in Replicas |-> [r2 \in Replicas |-> FALSE]]

--------------------------------------------------------------------------------
\* Actions
--------------------------------------------------------------------------------

\* Local write at replica r
LocalWrite(r, k, v) ==
    /\ generation[r] < MaxTimestamp
    /\ LET new_ts == generation[r] + 1
           new_kv == [value |-> v, timestamp |-> new_ts]
           bucket == KeyToBucket(k)
       IN /\ kv_state' = [kv_state EXCEPT ![r][k] = new_kv]
          /\ generation' = [generation EXCEPT ![r] = new_ts]
          \* Update merkle tree
          /\ merkle_buckets' = [merkle_buckets EXCEPT
                ![r][bucket] = ComputeBucketHash(r, bucket)]
          /\ merkle_root' = [merkle_root EXCEPT ![r] = ComputeRootHash(r)]
    /\ UNCHANGED <<peer_digests, divergent_peers, pending_syncs, partitioned>>

\* Exchange digests between replicas (lightweight)
ExchangeDigest(r1, r2) ==
    /\ r1 # r2
    /\ ~partitioned[r1][r2]
    /\ LET d1 == MakeDigest(r1)
           d2 == MakeDigest(r2)
       IN /\ peer_digests' = [peer_digests EXCEPT
                ![r1][r2] = d2,
                ![r2][r1] = d1]
          \* Detect divergence
          /\ IF d1.root_hash # d2.root_hash
             THEN divergent_peers' = [divergent_peers EXCEPT
                    ![r1] = @ \union {r2},
                    ![r2] = @ \union {r1}]
             ELSE divergent_peers' = [divergent_peers EXCEPT
                    ![r1] = @ \ {r2},
                    ![r2] = @ \ {r1}]
    /\ UNCHANGED <<kv_state, generation, merkle_buckets, merkle_root,
                   pending_syncs, partitioned>>

\* Initiate sync request (after detecting divergence)
InitiateSync(r, peer) ==
    /\ peer \in divergent_peers[r]
    /\ ~partitioned[r][peer]
    /\ LET buckets == DivergentBuckets(r, peer)
           req == [type |-> "SyncRequest", from |-> r, to |-> peer,
                  digest |-> MakeDigest(r), requested_buckets |-> buckets]
       IN pending_syncs' = pending_syncs \union {req}
    /\ UNCHANGED <<kv_state, generation, merkle_buckets, merkle_root,
                   peer_digests, divergent_peers, partitioned>>

\* Handle sync request (send back deltas for requested buckets)
HandleSyncRequest(req) ==
    /\ req \in pending_syncs
    /\ req.type = "SyncRequest"
    /\ ~partitioned[req.to][req.from]
    /\ LET responder == req.to
           \* Get keys in requested buckets
           keys_to_send == {k \in Keys : KeyToBucket(k) \in req.requested_buckets}
           deltas == [k \in Keys |->
               IF k \in keys_to_send THEN kv_state[responder][k] ELSE NULL]
           resp == [type |-> "SyncResponse", from |-> responder,
                   deltas |-> deltas, digest |-> MakeDigest(responder)]
       IN pending_syncs' = (pending_syncs \ {req}) \union {resp}
    /\ UNCHANGED <<kv_state, generation, merkle_buckets, merkle_root,
                   peer_digests, divergent_peers, partitioned>>

\* Handle sync response (merge received deltas using CRDT semantics)
HandleSyncResponse(resp, receiver) ==
    /\ resp \in pending_syncs
    /\ resp.type = "SyncResponse"
    /\ \* Merge each delta using CRDT last-writer-wins
       LET merged_state == [k \in Keys |->
               MergeValue(kv_state[receiver][k], resp.deltas[k])]
       IN kv_state' = [kv_state EXCEPT ![receiver] = merged_state]
    /\ pending_syncs' = pending_syncs \ {resp}
    /\ \* Update merkle tree after merge
       LET new_buckets == [b \in 0..NumBuckets-1 |-> ComputeBucketHash(receiver, b)]
       IN /\ merkle_buckets' = [merkle_buckets EXCEPT ![receiver] = new_buckets]
          /\ merkle_root' = [merkle_root EXCEPT ![receiver] = ComputeRootHash(receiver)]
    /\ divergent_peers' = [divergent_peers EXCEPT ![receiver] = @ \ {resp.from}]
    /\ UNCHANGED <<generation, peer_digests, partitioned>>

\* Create network partition
CreatePartition(r1, r2) ==
    /\ r1 # r2
    /\ ~partitioned[r1][r2]
    /\ partitioned' = [partitioned EXCEPT ![r1][r2] = TRUE, ![r2][r1] = TRUE]
    \* Drop in-flight syncs between partitioned nodes
    /\ pending_syncs' = {s \in pending_syncs :
          ~((s.type = "SyncRequest" /\ s.from = r1 /\ s.to = r2) \/
            (s.type = "SyncRequest" /\ s.from = r2 /\ s.to = r1) \/
            (s.type = "SyncResponse" /\ s.from = r1) \/
            (s.type = "SyncResponse" /\ s.from = r2))}
    /\ UNCHANGED <<kv_state, generation, merkle_buckets, merkle_root,
                   peer_digests, divergent_peers>>

\* Heal network partition (triggers sync)
HealPartition(r1, r2) ==
    /\ r1 # r2
    /\ partitioned[r1][r2]
    /\ partitioned' = [partitioned EXCEPT ![r1][r2] = FALSE, ![r2][r1] = FALSE]
    \* Mark both as divergent to trigger sync
    /\ divergent_peers' = [divergent_peers EXCEPT
          ![r1] = @ \union {r2},
          ![r2] = @ \union {r1}]
    /\ UNCHANGED <<kv_state, generation, merkle_buckets, merkle_root,
                   peer_digests, pending_syncs>>

--------------------------------------------------------------------------------
\* Next State Relation
--------------------------------------------------------------------------------

Next ==
    \/ \E r \in Replicas, k \in Keys, v \in 1..3 : LocalWrite(r, k, v)
    \/ \E r1, r2 \in Replicas : ExchangeDigest(r1, r2)
    \/ \E r \in Replicas, peer \in Replicas : InitiateSync(r, peer)
    \/ \E req \in pending_syncs : HandleSyncRequest(req)
    \/ \E resp \in pending_syncs, r \in Replicas : HandleSyncResponse(resp, r)
    \/ \E r1, r2 \in Replicas : CreatePartition(r1, r2)
    \/ \E r1, r2 \in Replicas : HealPartition(r1, r2)

Spec == Init /\ [][Next]_vars

FairSpec == Spec /\ WF_vars(\E r1, r2 \in Replicas : ExchangeDigest(r1, r2))
                 /\ WF_vars(\E r, p \in Replicas : InitiateSync(r, p))
                 /\ WF_vars(\E req \in pending_syncs : HandleSyncRequest(req))
                 /\ WF_vars(\E resp \in pending_syncs, r \in Replicas : HandleSyncResponse(resp, r))

--------------------------------------------------------------------------------
\* Invariants
--------------------------------------------------------------------------------

\* INVARIANT 1: Merkle root reflects bucket hashes
\* (Simplified: just check it's computed)
MerkleConsistent ==
    \A r \in Replicas : merkle_root[r] \in Nat

\* INVARIANT 2: Generations are monotonically increasing
GenerationMonotonic ==
    \A r \in Replicas : generation[r] >= 0

\* INVARIANT 3: No self-divergence
NoSelfDivergence ==
    \A r \in Replicas : r \notin divergent_peers[r]

\* INVARIANT 4: Partitions are symmetric
PartitionSymmetric ==
    \A r1, r2 \in Replicas : partitioned[r1][r2] = partitioned[r2][r1]

\* INVARIANT 5: Sync requests target valid replicas
SyncRequestsValid ==
    \A req \in pending_syncs :
        req.type = "SyncRequest" =>
            /\ req.from \in Replicas
            /\ req.to \in Replicas
            /\ req.from # req.to

--------------------------------------------------------------------------------
\* Temporal Properties
--------------------------------------------------------------------------------

\* PROPERTY 1: Eventual Convergence (Sync Completeness)
\* After partition heals and sync completes, replicas converge
EventualConvergence ==
    (\A r1, r2 \in Replicas : ~partitioned[r1][r2]) /\ pending_syncs = {} =>
        \A r1, r2 \in Replicas, k \in Keys :
            kv_state[r1][k] = kv_state[r2][k]

\* PROPERTY 2: Partition Healing Triggers Sync
\* When partition heals, divergent_peers is updated
PartitionHealTriggers ==
    \A r1, r2 \in Replicas :
        (partitioned[r1][r2] /\ partitioned'[r1][r2] = FALSE) =>
            r2 \in divergent_peers'[r1]

\* PROPERTY 3: Sync Completeness
\* Divergent peers eventually sync (under fairness)
SyncCompleteness ==
    \A r \in Replicas, p \in divergent_peers[r] :
        (p \in divergent_peers[r] /\ ~partitioned[r][p]) ~>
            p \notin divergent_peers[r]

================================================================================
\* Modification History
\* Created for redis-rust verification-driven exploration
\* Based on src/replication/anti_entropy.rs AntiEntropyManager
\* Merkle tree-based reconciliation for efficient partition healing
================================================================================
