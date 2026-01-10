//! TLS support via rustls
//!
//! Provides TLS encryption for Redis connections using rustls.

mod config;
mod stream;

pub use config::{TlsConfig, TlsError};
pub use stream::MaybeSecureStream;

// Re-export TlsAcceptor for convenience
pub use tokio_rustls::TlsAcceptor;
