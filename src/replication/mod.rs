pub mod lattice;
pub mod config;
pub mod gossip;
pub mod state;

pub use lattice::{LwwRegister, VectorClock, LamportClock, ReplicaId};
pub use config::{ReplicationConfig, ConsistencyLevel};
pub use state::{ReplicatedValue, ReplicationDelta};
