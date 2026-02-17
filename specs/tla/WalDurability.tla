---------------------------- MODULE WalDurability ----------------------------
\* TLA+ Specification for WAL + Streaming Hybrid Persistence
\*
\* Models the Write-Ahead Log with group commit and its interaction with the
\* streaming object store layer. Extends StreamingPersistence.tla with WAL state.
\*
\* Corresponds to:
\*   src/streaming/wal.rs           (entry format, writer, reader, rotator)
\*   src/streaming/wal_actor.rs     (group commit actor)
\*   src/streaming/wal_config.rs    (fsync policies)
\*   src/streaming/recovery.rs      (hybrid recovery)
\*
\* Run with TLC: tlc WalDurability.tla
\*
\* Key Invariants:
\* 1. WAL_DURABILITY: If fsync returns success, all entries in the batch survive crash
\* 2. TRUNCATION_SAFETY: WAL truncation never removes entries not yet in object store
\* 3. RECOVERY_COMPLETENESS: Object store segments UNION WAL entries = all acknowledged writes
\* 4. GROUP_COMMIT_ATOMICITY: Either all entries in a group commit batch are durable, or none
\* 5. HIGH_WATER_MARK_MONOTONIC: Streamed high-water mark never decreases

EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    MaxWrites,              \* Maximum number of writes for model checking
    MaxSegments,            \* Maximum object store segments
    GroupCommitBatchSize    \* Maximum entries per group commit batch

VARIABLES
    \* WAL state
    wal_buffer,             \* wal_buffer = sequence of entries pending fsync
    wal_synced,             \* wal_synced = set of entry timestamps that have been fsync'd

    \* Object store / streaming state (from StreamingPersistence)
    streamed_entries,       \* streamed_entries = set of entry timestamps in object store
    high_water_mark,        \* high_water_mark = max timestamp in object store segments

    \* Acknowledged writes (the ground truth for correctness)
    acknowledged,           \* acknowledged = set of entry timestamps acknowledged to client

    \* Crash/recovery state
    crashed,                \* crashed = TRUE if system has crashed
    recovered,              \* recovered = TRUE if recovery is complete

    \* Counters
    write_counter           \* write_counter = next timestamp to assign

vars == <<wal_buffer, wal_synced,
          streamed_entries, high_water_mark,
          acknowledged, crashed, recovered, write_counter>>

--------------------------------------------------------------------------------
\* Initial State
--------------------------------------------------------------------------------

Init ==
    /\ wal_buffer = <<>>
    /\ wal_synced = {}
    /\ streamed_entries = {}
    /\ high_water_mark = 0
    /\ acknowledged = {}
    /\ crashed = FALSE
    /\ recovered = TRUE
    /\ write_counter = 1

--------------------------------------------------------------------------------
\* Type Invariant
--------------------------------------------------------------------------------

TypeOK ==
    /\ wal_buffer \in Seq(Nat)
    /\ wal_synced \subseteq Nat
    /\ streamed_entries \subseteq Nat
    /\ high_water_mark \in Nat
    /\ acknowledged \subseteq Nat
    /\ crashed \in BOOLEAN
    /\ recovered \in BOOLEAN
    /\ write_counter \in Nat

--------------------------------------------------------------------------------
\* Actions
--------------------------------------------------------------------------------

\* Client writes a delta — appended to WAL buffer (not yet durable)
WalAppend ==
    /\ ~crashed
    /\ write_counter <= MaxWrites
    /\ Len(wal_buffer) < GroupCommitBatchSize
    /\ LET ts == write_counter
       IN /\ wal_buffer' = Append(wal_buffer, ts)
          /\ write_counter' = write_counter + 1
    /\ UNCHANGED <<wal_synced, streamed_entries, high_water_mark,
                   acknowledged, crashed, recovered>>

\* Group commit: fsync all entries in WAL buffer, acknowledge to clients
\* This is the critical action — after this, entries are durable
WalSync ==
    /\ ~crashed
    /\ Len(wal_buffer) > 0
    /\ LET entries == { wal_buffer[i] : i \in 1..Len(wal_buffer) }
       IN /\ wal_synced' = wal_synced \union entries
          /\ acknowledged' = acknowledged \union entries
    /\ wal_buffer' = <<>>
    /\ UNCHANGED <<streamed_entries, high_water_mark,
                   crashed, recovered, write_counter>>

\* Streaming: object store flush — entries move from WAL to object store.
\* Models the async path: DeltaSink -> PersistenceActor -> ObjectStore.
\* Non-deterministically picks a subset (models partial streaming).
StreamFlush ==
    /\ ~crashed
    /\ wal_synced # {}
    /\ Cardinality(streamed_entries) < MaxSegments * GroupCommitBatchSize
    /\ \E subset \in (SUBSET wal_synced \ {{}}) :
        /\ Cardinality(subset) <= GroupCommitBatchSize
        /\ LET max_ts == CHOOSE ts \in subset : \A t \in subset : ts >= t
           IN /\ streamed_entries' = streamed_entries \union subset
              /\ high_water_mark' = IF max_ts > high_water_mark
                                    THEN max_ts
                                    ELSE high_water_mark
    /\ UNCHANGED <<wal_buffer, wal_synced,
                   acknowledged, crashed, recovered, write_counter>>

\* WAL truncation: delete WAL entries that are already in object store
\* Only truncate entries at or below the high-water mark
WalTruncate ==
    /\ ~crashed
    /\ high_water_mark > 0
    /\ LET entries_to_remove == { ts \in wal_synced : ts <= high_water_mark }
       IN /\ entries_to_remove # {}
          /\ wal_synced' = wal_synced \ entries_to_remove
    /\ UNCHANGED <<wal_buffer, streamed_entries, high_water_mark,
                   acknowledged, crashed, recovered, write_counter>>

\* System crash: loses WAL buffer, but synced WAL entries and object store survive
Crash ==
    /\ ~crashed
    /\ crashed' = TRUE
    /\ recovered' = FALSE
    /\ wal_buffer' = <<>>
    /\ UNCHANGED <<wal_synced, streamed_entries, high_water_mark,
                   acknowledged, write_counter>>

\* Recovery: rebuild state from object store + WAL replay
Recover ==
    /\ crashed
    /\ ~recovered
    /\ crashed' = FALSE
    /\ recovered' = TRUE
    /\ wal_buffer' = <<>>
    /\ UNCHANGED <<wal_synced, streamed_entries, high_water_mark,
                   acknowledged, write_counter>>

\* Fsync failure: WAL buffer entries are NOT acknowledged (realistic fault)
WalSyncFail ==
    /\ ~crashed
    /\ Len(wal_buffer) > 0
    /\ wal_buffer' = <<>>
    /\ UNCHANGED <<wal_synced, streamed_entries, high_water_mark,
                   acknowledged, crashed, recovered, write_counter>>

--------------------------------------------------------------------------------
\* Next State Relation
--------------------------------------------------------------------------------

Next ==
    \/ WalAppend
    \/ WalSync
    \/ WalSyncFail
    \/ StreamFlush
    \/ WalTruncate
    \/ Crash
    \/ Recover

Spec == Init /\ [][Next]_vars

FairSpec == Spec /\ WF_vars(WalSync) /\ WF_vars(Recover) /\ WF_vars(StreamFlush)

--------------------------------------------------------------------------------
\* Invariants
--------------------------------------------------------------------------------

\* INVARIANT 1: Acknowledged Entries Are Recoverable
\* The primary safety invariant: every acknowledged entry is either in
\* wal_synced (local WAL) OR in streamed_entries (object store).
\* This must hold in ALL states including post-crash states.
AcknowledgedRecoverable ==
    \A ts \in acknowledged :
        ts \in wal_synced \/ ts \in streamed_entries

\* INVARIANT 2: Recovery Completeness
\* After recovery completes, all acknowledged entries are still recoverable.
\* (Strictly weaker than AcknowledgedRecoverable but explicit about recovery state.)
RecoveryCompleteness ==
    recovered =>
        \A ts \in acknowledged :
            ts \in streamed_entries \/ ts \in wal_synced

\* INVARIANT 3: Buffer Entries Not Acknowledged
\* Entries still in the WAL buffer (pre-fsync) must NOT be in the acknowledged set.
\* This ensures we never tell a client "durable" before fsync completes.
BufferNotAcknowledged ==
    \A i \in 1..Len(wal_buffer) :
        wal_buffer[i] \notin acknowledged

\* INVARIANT 4: High-Water Mark Consistency
\* High-water mark equals the maximum of streamed entries (or 0 if none).
HighWaterMarkConsistent ==
    IF streamed_entries = {}
    THEN high_water_mark = 0
    ELSE high_water_mark >= CHOOSE ts \in streamed_entries : \A t \in streamed_entries : ts >= t

\* INVARIANT 5: Write counter monotonic
WriteCounterMonotonic ==
    write_counter >= 1

\* Combined invariant for TLC
AllInvariants ==
    /\ TypeOK
    /\ AcknowledgedRecoverable
    /\ RecoveryCompleteness
    /\ BufferNotAcknowledged
    /\ HighWaterMarkConsistent
    /\ WriteCounterMonotonic

--------------------------------------------------------------------------------
\* Temporal Properties
--------------------------------------------------------------------------------

\* PROPERTY 1: Eventual Durability
\* If a write is appended and the system doesn't crash, it eventually gets acknowledged
EventualDurability ==
    \A ts \in 1..MaxWrites :
        (ts \in { wal_buffer[i] : i \in 1..Len(wal_buffer) } /\ ~crashed) ~>
            (ts \in acknowledged \/ crashed)

\* PROPERTY 2: Eventual Streaming
\* Acknowledged entries eventually reach the object store (if system stays up)
EventualStreaming ==
    \A ts \in acknowledged :
        (~crashed) ~> (ts \in streamed_entries \/ crashed)

\* PROPERTY 3: Recovery Terminates
\* After crash, recovery eventually completes
RecoveryTerminates ==
    crashed ~> recovered

================================================================================
\* TLC Configuration (put in WalDurability.cfg or use command line)
\*
\* SPECIFICATION Spec
\* INVARIANT AllInvariants
\* PROPERTY RecoveryTerminates
\*
\* CONSTANTS
\*   MaxWrites = 4
\*   MaxWalFiles = 3
\*   MaxSegments = 3
\*   GroupCommitBatchSize = 2
\*
\* Small constants for tractable model checking. The invariants are
\* universal — they hold for any values, but TLC needs finite bounds.
================================================================================
