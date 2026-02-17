//! Streaming Object Store Persistence
//!
//! Provides durable persistence using object storage (S3/local filesystem)
//! with streaming delta writes. Deltas are buffered in memory and flushed
//! periodically to immutable segment files in the object store.
//!
//! ## Architecture
//!
//! ```text
//! Delta → WriteBuffer → Segment → ObjectStore
//!                ↓
//!          (250ms flush)
//! ```
//!
//! ## Key Features
//!
//! - **Stateless nodes**: Any node can recover from object store
//! - **CRDT-safe recovery**: Idempotent replay of deltas
//! - **Batched writes**: Efficient 250ms flush interval
//! - **Checksummed segments**: CRC32 validation

pub mod checkpoint;
pub mod clock;
pub mod compaction;
pub mod compaction_dst;
pub mod config;
pub mod delta_sink;
pub mod dst;
pub mod integration;
pub mod manifest;
pub mod object_store;
pub mod persistence;
pub mod recovery;
#[cfg(feature = "s3")]
pub mod s3_store;
pub mod segment;
pub mod simulated_store;
pub mod wal;
pub mod wal_actor;
pub mod wal_config;
pub mod wal_dst;
pub mod wal_store;
pub mod write_buffer;

pub use checkpoint::{
    CheckpointConfig, CheckpointData, CheckpointError, CheckpointManager, CheckpointReader,
    CheckpointResult, CheckpointWriter,
};
pub use clock::{ProductionClock, SimulatedClock, StreamingClock, StreamingTimestamp};
pub use compaction::{
    CompactionConfig, CompactionError, CompactionResult, CompactionStats, CompactionWorker,
    CompactionWorkerHandle, Compactor,
};
pub use compaction_dst::{
    run_compaction_dst_batch, summarize_compaction_batch, CompactionDSTConfig,
    CompactionDSTHarness, CompactionDSTResult, CompactionOperation, CompactionOutcome,
    CompactionWorkload,
};
#[cfg(feature = "s3")]
pub use config::S3Config;
pub use config::{
    CheckpointConfig as CheckpointConfigSerde, CompactionConfig as CompactionConfigSerde,
    ObjectStoreType, StreamingConfig, WriteBufferConfig,
};
pub use delta_sink::{
    delta_sink_channel, DeltaSinkError, DeltaSinkReceiver, DeltaSinkSender,
    PersistenceWorker as DeltaSinkPersistenceWorker,
    PersistenceWorkerHandle as DeltaSinkPersistenceWorkerHandle,
};
pub use dst::{
    run_dst_batch, summarize_batch, OperationOutcome, StreamingDSTConfig, StreamingDSTHarness,
    StreamingDSTResult, StreamingOperation, StreamingWorkload,
};
pub use integration::{
    create_integration, IntegrationError, StreamingIntegration, StreamingIntegrationTrait,
    WorkerHandles,
};
pub use manifest::{CheckpointInfo, Manifest, ManifestError, ManifestManager, SegmentInfo};
pub use object_store::{InMemoryObjectStore, LocalFsObjectStore};
pub use object_store::{ListResult, ObjectMeta, ObjectStore, ObjectStoreError};
pub use persistence::{
    FlushResult, PersistenceError, PersistenceStats, PersistenceWorker, PersistenceWorkerHandle,
    StreamingPersistence,
};
pub use recovery::{
    RecoveredState, RecoveryError, RecoveryManager, RecoveryPhase, RecoveryProgress, RecoveryStats,
};
#[cfg(feature = "s3")]
pub use s3_store::S3ObjectStore;
pub use segment::{
    Compression, Segment, SegmentError, SegmentFooter, SegmentHeader, SegmentReader, SegmentWriter,
};
pub use simulated_store::{SimulatedObjectStore, SimulatedStoreConfig, SimulatedStoreStats};
pub use wal::{WalEntry, WalReader, WalRotator, WalWriter};
pub use wal_actor::{spawn_wal_actor, WalActorHandle, WalMessage};
pub use wal_config::{FsyncPolicy, WalConfig};
pub use wal_dst::{
    run_wal_dst_batch, summarize_wal_dst_batch, WalDSTConfig, WalDSTHarness, WalDSTResult,
};
pub use wal_store::{
    InMemoryWalStore, LocalWalStore, SimulatedWalStore, SimulatedWalStoreConfig,
    SimulatedWalStoreStats, WalError, WalFileReader, WalFileWriter, WalStore,
};
pub use write_buffer::{
    FlushWorker, FlushWorkerHandle, WriteBuffer, WriteBufferError, WriteBufferStats,
};
