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

pub mod object_store;
pub mod segment;
pub mod config;
pub mod write_buffer;
pub mod delta_sink;
pub mod simulated_store;
pub mod manifest;
pub mod recovery;
pub mod persistence;
pub mod checkpoint;
pub mod compaction;
pub mod integration;
pub mod clock;
pub mod dst;
pub mod compaction_dst;
#[cfg(feature = "s3")]
pub mod s3_store;

pub use object_store::{ObjectStore, ObjectMeta, ListResult, ObjectStoreError};
pub use object_store::{InMemoryObjectStore, LocalFsObjectStore};
pub use segment::{
    Segment, SegmentWriter, SegmentReader, SegmentHeader, SegmentFooter,
    SegmentError, Compression,
};
pub use config::{
    StreamingConfig, WriteBufferConfig, ObjectStoreType,
    CheckpointConfig as CheckpointConfigSerde, CompactionConfig as CompactionConfigSerde,
};
pub use write_buffer::{WriteBuffer, WriteBufferError, WriteBufferStats, FlushWorker, FlushWorkerHandle};
pub use delta_sink::{
    DeltaSinkSender, DeltaSinkReceiver, DeltaSinkError,
    delta_sink_channel, PersistenceWorker as DeltaSinkPersistenceWorker,
    PersistenceWorkerHandle as DeltaSinkPersistenceWorkerHandle,
};
pub use simulated_store::{SimulatedObjectStore, SimulatedStoreConfig, SimulatedStoreStats};
pub use manifest::{Manifest, ManifestManager, ManifestError, SegmentInfo, CheckpointInfo};
pub use recovery::{RecoveryManager, RecoveryError, RecoveredState, RecoveryProgress, RecoveryPhase, RecoveryStats};
pub use persistence::{
    StreamingPersistence, PersistenceError, PersistenceStats, FlushResult,
    PersistenceWorker, PersistenceWorkerHandle,
};
pub use checkpoint::{
    CheckpointConfig, CheckpointError, CheckpointWriter, CheckpointReader,
    CheckpointManager, CheckpointResult, CheckpointData,
};
pub use compaction::{
    CompactionConfig, CompactionError, CompactionResult, CompactionStats,
    Compactor, CompactionWorker, CompactionWorkerHandle,
};
pub use integration::{
    StreamingIntegration, IntegrationError, WorkerHandles,
    StreamingIntegrationTrait, create_integration,
};
pub use clock::{StreamingClock, StreamingTimestamp, ProductionClock, SimulatedClock};
pub use dst::{
    StreamingDSTConfig, StreamingDSTHarness, StreamingDSTResult,
    StreamingWorkload, StreamingOperation, OperationOutcome,
    run_dst_batch, summarize_batch,
};
pub use compaction_dst::{
    CompactionDSTConfig, CompactionDSTHarness, CompactionDSTResult,
    CompactionWorkload, CompactionOperation, CompactionOutcome,
    run_compaction_dst_batch, summarize_compaction_batch,
};
#[cfg(feature = "s3")]
pub use s3_store::S3ObjectStore;
#[cfg(feature = "s3")]
pub use config::S3Config;
