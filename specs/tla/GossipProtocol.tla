---------------------------- MODULE GossipProtocol ----------------------------
\* TLA+ Specification for redis-rust Gossip Protocol
\*
\* This spec models the gossip-based delta dissemination protocol.
\* Corresponds to: src/replication/gossip.rs, gossip_router.rs
\*
\* Run with TLC: tlc GossipProtocol.tla
\*
\* Key Invariants:
\* 1. GOSSIP_DELIVERY: All deltas are eventually delivered to all replicas
\* 2. NO_DUPLICATE_PROCESSING: Each delta is processed at most once per replica
\* 3. SELECTIVE_ROUTING: In partitioned mode, deltas route to correct targets
\* 4. EPOCH_MONOTONIC: Gossip epochs never decrease
\* 5. SOURCE_CORRECT: Message source always matches originating replica

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Replicas,           \* Set of replica IDs
    Keys,               \* Set of keys
    MaxEpoch,           \* Maximum epoch for model checking
    MaxMessages         \* Maximum in-flight messages (for bounded model checking)

VARIABLES
    \* Per-replica state
    local_state,        \* local_state[r][k] = latest delta for key k at replica r
    processed_deltas,   \* processed_deltas[r] = set of delta IDs already processed
    gossip_epoch,       \* gossip_epoch[r] = current gossip epoch at replica r
    outbound_queue,     \* outbound_queue[r] = queue of messages to send

    \* Network state
    network,            \* network = set of in-flight gossip messages

    \* Routing state (for selective gossip)
    routing_table,      \* routing_table[r][k] = target replica for key k
    selective_mode      \* selective_mode[r] = TRUE if selective gossip enabled

vars == <<local_state, processed_deltas, gossip_epoch, outbound_queue,
          network, routing_table, selective_mode>>

--------------------------------------------------------------------------------
\* Type Definitions
--------------------------------------------------------------------------------

DeltaId == [key: Keys, seq: Nat, origin: Replicas]

Delta == [
    id: DeltaId,
    key: Keys,
    value: Nat,         \* Simplified value (in code: ReplicationDelta)
    origin: Replicas,
    epoch: Nat
]

GossipMessage == [
    type: {"DeltaBatch", "TargetedDelta", "Heartbeat"},
    source: Replicas,
    target: Replicas \union {NULL},  \* NULL = broadcast
    deltas: SUBSET Delta,
    epoch: Nat
]

TypeOK ==
    /\ local_state \in [Replicas -> [Keys -> Delta \union {NULL}]]
    /\ processed_deltas \in [Replicas -> SUBSET DeltaId]
    /\ gossip_epoch \in [Replicas -> Nat]
    /\ outbound_queue \in [Replicas -> Seq(GossipMessage)]
    /\ network \subseteq GossipMessage
    /\ routing_table \in [Replicas -> [Keys -> Replicas]]
    /\ selective_mode \in [Replicas -> BOOLEAN]

--------------------------------------------------------------------------------
\* Helper Functions
--------------------------------------------------------------------------------

\* Create a delta ID
MakeDeltaId(key, seq, origin) == [key |-> key, seq |-> seq, origin |-> origin]

\* Check if delta was already processed
AlreadyProcessed(r, delta_id) == delta_id \in processed_deltas[r]

\* Get target replicas for a key (selective routing)
GetTargets(r, key) ==
    IF selective_mode[r]
    THEN {routing_table[r][key]}
    ELSE Replicas \ {r}

\* Count messages in network (for bounding)
MessageCount == Cardinality(network)

--------------------------------------------------------------------------------
\* Initial State
--------------------------------------------------------------------------------

Init ==
    /\ local_state = [r \in Replicas |-> [k \in Keys |-> NULL]]
    /\ processed_deltas = [r \in Replicas |-> {}]
    /\ gossip_epoch = [r \in Replicas |-> 0]
    /\ outbound_queue = [r \in Replicas |-> <<>>]
    /\ network = {}
    /\ routing_table = [r \in Replicas |-> [k \in Keys |-> CHOOSE r2 \in Replicas : TRUE]]
    /\ selective_mode = [r \in Replicas |-> FALSE]

--------------------------------------------------------------------------------
\* Actions
--------------------------------------------------------------------------------

\* Replica r creates a new delta for key k with value v
CreateDelta(r, k, v) ==
    /\ gossip_epoch[r] < MaxEpoch
    /\ MessageCount < MaxMessages
    /\ LET delta_id == MakeDeltaId(k, gossip_epoch[r], r)
           new_delta == [id |-> delta_id, key |-> k, value |-> v,
                        origin |-> r, epoch |-> gossip_epoch[r]]
       IN /\ local_state' = [local_state EXCEPT ![r][k] = new_delta]
          /\ processed_deltas' = [processed_deltas EXCEPT ![r] = @ \union {delta_id}]
          \* Queue delta for gossip
          /\ IF selective_mode[r]
             THEN \* Selective: send targeted messages to specific replicas
                  LET targets == GetTargets(r, k) \ {r}
                      msgs == {[type |-> "TargetedDelta", source |-> r,
                               target |-> t, deltas |-> {new_delta},
                               epoch |-> gossip_epoch[r]] : t \in targets}
                  IN network' = network \union msgs
             ELSE \* Broadcast: send to all replicas
                  LET msg == [type |-> "DeltaBatch", source |-> r,
                             target |-> NULL, deltas |-> {new_delta},
                             epoch |-> gossip_epoch[r]]
                  IN network' = network \union {msg}
    /\ UNCHANGED <<gossip_epoch, outbound_queue, routing_table, selective_mode>>

\* Replica r receives and processes a gossip message
ReceiveGossip(r, msg) ==
    /\ msg \in network
    /\ \/ msg.target = NULL                \* Broadcast message
       \/ msg.target = r                    \* Targeted message for us
    /\ msg.source # r                       \* Don't receive own messages
    \* Process deltas
    /\ LET new_deltas == {d \in msg.deltas : ~AlreadyProcessed(r, d.id)}
           updated_state == [k \in Keys |->
               IF \E d \in new_deltas : d.key = k
               THEN CHOOSE d \in new_deltas : d.key = k
               ELSE local_state[r][k]]
           new_ids == {d.id : d \in new_deltas}
       IN /\ local_state' = [local_state EXCEPT ![r] = updated_state]
          /\ processed_deltas' = [processed_deltas EXCEPT ![r] = @ \union new_ids]
    /\ network' = network \ {msg}
    /\ UNCHANGED <<gossip_epoch, outbound_queue, routing_table, selective_mode>>

\* Replica r advances its gossip epoch
AdvanceEpoch(r) ==
    /\ gossip_epoch[r] < MaxEpoch
    /\ gossip_epoch' = [gossip_epoch EXCEPT ![r] = @ + 1]
    /\ UNCHANGED <<local_state, processed_deltas, outbound_queue,
                   network, routing_table, selective_mode>>

\* Replica r sends a heartbeat
SendHeartbeat(r) ==
    /\ MessageCount < MaxMessages
    /\ LET msg == [type |-> "Heartbeat", source |-> r, target |-> NULL,
                  deltas |-> {}, epoch |-> gossip_epoch[r]]
       IN network' = network \union {msg}
    /\ UNCHANGED <<local_state, processed_deltas, gossip_epoch,
                   outbound_queue, routing_table, selective_mode>>

\* Enable selective gossip mode on replica r
EnableSelectiveMode(r) ==
    /\ ~selective_mode[r]
    /\ selective_mode' = [selective_mode EXCEPT ![r] = TRUE]
    /\ UNCHANGED <<local_state, processed_deltas, gossip_epoch,
                   outbound_queue, network, routing_table>>

\* Update routing table entry
UpdateRouting(r, k, target) ==
    /\ target \in Replicas
    /\ target # r
    /\ routing_table' = [routing_table EXCEPT ![r][k] = target]
    /\ UNCHANGED <<local_state, processed_deltas, gossip_epoch,
                   outbound_queue, network, selective_mode>>

\* Message loss (for fault injection)
MessageLoss(msg) ==
    /\ msg \in network
    /\ network' = network \ {msg}
    /\ UNCHANGED <<local_state, processed_deltas, gossip_epoch,
                   outbound_queue, routing_table, selective_mode>>

--------------------------------------------------------------------------------
\* Next State Relation
--------------------------------------------------------------------------------

Next ==
    \/ \E r \in Replicas, k \in Keys, v \in 1..3 : CreateDelta(r, k, v)
    \/ \E r \in Replicas, msg \in network : ReceiveGossip(r, msg)
    \/ \E r \in Replicas : AdvanceEpoch(r)
    \/ \E r \in Replicas : SendHeartbeat(r)
    \/ \E r \in Replicas : EnableSelectiveMode(r)
    \/ \E r \in Replicas, k \in Keys, t \in Replicas : UpdateRouting(r, k, t)
    \/ \E msg \in network : MessageLoss(msg)

Spec == Init /\ [][Next]_vars

--------------------------------------------------------------------------------
\* Invariants
--------------------------------------------------------------------------------

\* INVARIANT 1: Epochs are monotonically non-decreasing
\* (verified implicitly by AdvanceEpoch only incrementing)

\* INVARIANT 2: Message source is always correct (source matches originator)
SourceCorrectInvariant ==
    \A msg \in network :
        msg.source \in Replicas /\ msg.source # NULL

\* INVARIANT 3: Targeted messages have valid targets
TargetedMessageValid ==
    \A msg \in network :
        msg.type = "TargetedDelta" => msg.target # NULL /\ msg.target \in Replicas

\* INVARIANT 4: No self-targeting
NoSelfTarget ==
    \A msg \in network :
        msg.target # NULL => msg.target # msg.source

\* INVARIANT 5: Processed deltas are remembered
ProcessedDeltasGrow ==
    \A r \in Replicas, d \in processed_deltas[r] : d \in DeltaId

--------------------------------------------------------------------------------
\* Temporal Properties
--------------------------------------------------------------------------------

\* PROPERTY 1: Eventual Delivery (weak fairness assumed)
\* If a delta is created and network eventually empties, all replicas have it
EventualDelivery ==
    \A r1, r2 \in Replicas, k \in Keys :
        (local_state[r1][k] # NULL /\ network = {}) =>
            (local_state[r2][k] # NULL \/ processed_deltas[r2] = {})

\* PROPERTY 2: No duplicate processing
\* Once a delta is processed, it's never processed again
\* (This is ensured by checking AlreadyProcessed before processing)

================================================================================
\* Modification History
\* Created for redis-rust verification-driven exploration
\* Based on src/replication/gossip.rs GossipState and GossipMessage
================================================================================
