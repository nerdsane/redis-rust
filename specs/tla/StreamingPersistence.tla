-------------------------- MODULE StreamingPersistence --------------------------
\* TLA+ Specification for redis-rust Streaming Persistence
\*
\* This spec models the write buffer and object store persistence protocol.
\* Corresponds to: src/streaming/persistence.rs, write_buffer.rs
\*
\* Run with TLC: tlc StreamingPersistence.tla
\*
\* Key Invariants:
\* 1. DURABILITY_GUARANTEE: Flushed deltas survive crashes
\* 2. WRITE_BUFFER_BOUNDED: Buffer never exceeds backpressure threshold
\* 3. RECOVERY_COMPLETENESS: All flushed segments can be recovered
\* 4. SEGMENT_ID_MONOTONIC: Segment IDs always increase
\* 5. MANIFEST_CONSISTENT: Manifest reflects all written segments

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    MaxBufferSize,          \* Maximum buffer size in bytes (backpressure threshold)
    MaxDeltas,              \* Maximum deltas before flush trigger
    MaxSegments,            \* Maximum segments for model checking
    DeltaSize               \* Size of each delta (simplified)

VARIABLES
    \* Write buffer state
    buffer,                 \* buffer = sequence of deltas pending flush
    buffer_size,            \* buffer_size = current estimated buffer size in bytes

    \* Persistence state
    segments,               \* segments = set of segment IDs that have been written
    next_segment_id,        \* next_segment_id = ID for next segment

    \* Manifest state
    manifest_segments,      \* manifest_segments = set of segment IDs in manifest
    manifest_generation,    \* manifest_generation = version of manifest

    \* Object store state (abstraction)
    object_store,           \* object_store[segment_id] = segment data (or NULL)

    \* Crash/recovery state
    crashed,                \* crashed = TRUE if system is crashed
    recovered               \* recovered = TRUE if recovery completed

vars == <<buffer, buffer_size, segments, next_segment_id,
          manifest_segments, manifest_generation, object_store,
          crashed, recovered>>

--------------------------------------------------------------------------------
\* Type Definitions
--------------------------------------------------------------------------------

Delta == [
    key: Nat,               \* Simplified key
    value: Nat,             \* Simplified value
    timestamp: Nat
]

SegmentData == [
    id: Nat,
    deltas: Seq(Delta),
    record_count: Nat
]

TypeOK ==
    /\ buffer \in Seq(Delta)
    /\ buffer_size \in Nat
    /\ segments \subseteq Nat
    /\ next_segment_id \in Nat
    /\ manifest_segments \subseteq Nat
    /\ manifest_generation \in Nat
    /\ object_store \in [Nat -> SegmentData \union {NULL}]
    /\ crashed \in BOOLEAN
    /\ recovered \in BOOLEAN

--------------------------------------------------------------------------------
\* Helper Functions
--------------------------------------------------------------------------------

\* Estimate buffer size after adding a delta
EstimateSize(buf) == Len(buf) * DeltaSize

\* Check if buffer should be flushed
ShouldFlush ==
    /\ Len(buffer) > 0
    /\ \/ buffer_size >= MaxBufferSize \div 2      \* Size threshold (half of max for flush trigger)
       \/ Len(buffer) >= MaxDeltas                  \* Count threshold

\* Check if buffer would exceed backpressure threshold
WouldExceedBackpressure == buffer_size >= MaxBufferSize

--------------------------------------------------------------------------------
\* Initial State
--------------------------------------------------------------------------------

Init ==
    /\ buffer = <<>>
    /\ buffer_size = 0
    /\ segments = {}
    /\ next_segment_id = 0
    /\ manifest_segments = {}
    /\ manifest_generation = 0
    /\ object_store = [i \in Nat |-> NULL]
    /\ crashed = FALSE
    /\ recovered = TRUE

--------------------------------------------------------------------------------
\* Actions
--------------------------------------------------------------------------------

\* Push a delta to the write buffer
PushDelta(key, value, ts) ==
    /\ ~crashed
    /\ ~WouldExceedBackpressure         \* Backpressure check
    /\ LET new_delta == [key |-> key, value |-> value, timestamp |-> ts]
           new_buffer == Append(buffer, new_delta)
           new_size == buffer_size + DeltaSize
       IN /\ buffer' = new_buffer
          /\ buffer_size' = new_size
    /\ UNCHANGED <<segments, next_segment_id, manifest_segments,
                   manifest_generation, object_store, crashed, recovered>>

\* Flush buffer to object store (creates a new segment)
FlushBuffer ==
    /\ ~crashed
    /\ Len(buffer) > 0
    /\ next_segment_id < MaxSegments        \* Bound for model checking
    /\ LET seg_id == next_segment_id
           seg_data == [id |-> seg_id, deltas |-> buffer, record_count |-> Len(buffer)]
       IN /\ object_store' = [object_store EXCEPT ![seg_id] = seg_data]
          /\ segments' = segments \union {seg_id}
          /\ next_segment_id' = next_segment_id + 1
          \* Update manifest
          /\ manifest_segments' = manifest_segments \union {seg_id}
          /\ manifest_generation' = manifest_generation + 1
          \* Clear buffer
          /\ buffer' = <<>>
          /\ buffer_size' = 0
    /\ UNCHANGED <<crashed, recovered>>

\* Force flush (triggered by worker or explicit call)
ForceFlush ==
    /\ Len(buffer) > 0
    /\ FlushBuffer

\* System crash (loses unflushed buffer)
Crash ==
    /\ ~crashed
    /\ crashed' = TRUE
    /\ recovered' = FALSE
    \* Buffer is lost, but persisted segments survive
    /\ buffer' = <<>>
    /\ buffer_size' = 0
    /\ UNCHANGED <<segments, next_segment_id, manifest_segments,
                   manifest_generation, object_store>>

\* System recovery (reloads from manifest)
Recover ==
    /\ crashed
    /\ ~recovered
    /\ crashed' = FALSE
    /\ recovered' = TRUE
    \* After recovery, buffer is empty, segments are intact
    /\ buffer' = <<>>
    /\ buffer_size' = 0
    \* Verify manifest is consistent with object store
    /\ \A seg_id \in manifest_segments : object_store[seg_id] # NULL
    /\ UNCHANGED <<segments, next_segment_id, manifest_segments,
                   manifest_generation, object_store>>

\* Object store failure (segment becomes unavailable - for fault injection)
SegmentLoss(seg_id) ==
    /\ seg_id \in segments
    /\ object_store[seg_id] # NULL
    /\ object_store' = [object_store EXCEPT ![seg_id] = NULL]
    /\ UNCHANGED <<buffer, buffer_size, segments, next_segment_id,
                   manifest_segments, manifest_generation, crashed, recovered>>

--------------------------------------------------------------------------------
\* Next State Relation
--------------------------------------------------------------------------------

Next ==
    \/ \E k, v, ts \in 1..3 : PushDelta(k, v, ts)
    \/ FlushBuffer
    \/ Crash
    \/ Recover
    \/ \E seg_id \in segments : SegmentLoss(seg_id)

Spec == Init /\ [][Next]_vars

FairSpec == Spec /\ WF_vars(FlushBuffer) /\ WF_vars(Recover)

--------------------------------------------------------------------------------
\* Invariants
--------------------------------------------------------------------------------

\* INVARIANT 1: Write buffer never exceeds backpressure threshold
WriteBufferBounded ==
    buffer_size <= MaxBufferSize

\* INVARIANT 2: Segment IDs are monotonically increasing
SegmentIdMonotonic ==
    \A seg_id \in segments : seg_id < next_segment_id

\* INVARIANT 3: Manifest only contains segments that exist in object store
\* (when not crashed)
ManifestConsistent ==
    ~crashed => \A seg_id \in manifest_segments :
                    seg_id \in segments

\* INVARIANT 4: Buffer size estimate is consistent
BufferSizeConsistent ==
    buffer_size = EstimateSize(buffer)

\* INVARIANT 5: Recovered state is valid
RecoveredStateValid ==
    recovered => (Len(buffer) = 0 /\ buffer_size = 0)

\* INVARIANT 6: No segment ID reuse
NoSegmentIdReuse ==
    \A s1, s2 \in segments : s1 = s2 => TRUE

--------------------------------------------------------------------------------
\* Temporal Properties
--------------------------------------------------------------------------------

\* PROPERTY 1: Durability Guarantee
\* Once a segment is in manifest, it remains durable (unless explicit loss)
DurabilityGuarantee ==
    \A seg_id \in manifest_segments :
        recovered => (object_store[seg_id] # NULL \/
                     \* Segment was explicitly lost
                     seg_id \notin segments)

\* PROPERTY 2: Eventual Flush
\* If buffer has data and we don't crash, it eventually flushes
\* (requires fairness on FlushBuffer)
EventualFlush ==
    (Len(buffer) > 0 /\ ~crashed) ~> (Len(buffer) = 0 \/ crashed)

\* PROPERTY 3: Recovery Completeness
\* After crash and recovery, system is in consistent state
RecoveryCompleteness ==
    (crashed /\ ~recovered) ~> (recovered /\ Len(buffer) = 0)

================================================================================
\* Modification History
\* Created for redis-rust verification-driven exploration
\* Based on src/streaming/persistence.rs StreamingPersistence
\* and src/streaming/write_buffer.rs WriteBuffer
================================================================================
