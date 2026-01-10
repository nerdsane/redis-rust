//! Security features: TLS encryption and ACL authentication
//!
//! This module provides optional security features:
//! - `tls` feature: TLS encryption via rustls
//! - `acl` feature: Redis 6.0+ compatible ACL system
//! - `security` feature: Both TLS and ACL

#[cfg(feature = "acl")]
pub mod acl;

#[cfg(feature = "tls")]
pub mod tls;

// Re-export main types for convenience
#[cfg(feature = "acl")]
pub use acl::{AclError, AclManager, AclUser};

#[cfg(feature = "tls")]
pub use tls::{MaybeSecureStream, TlsConfig, TlsError};

// No-op ACL manager when ACL feature is disabled
#[cfg(not(feature = "acl"))]
pub mod acl_noop {
    use std::sync::Arc;

    /// No-op ACL user - always permitted
    #[derive(Debug, Clone)]
    pub struct AclUser {
        pub name: String,
    }

    impl AclUser {
        pub fn default_user() -> Self {
            Self {
                name: "default".to_string(),
            }
        }
    }

    /// No-op ACL manager - always permits everything
    #[derive(Debug, Clone, Default)]
    pub struct AclManager;

    impl AclManager {
        pub fn new() -> Self {
            Self
        }

        /// Always returns the default user (no authentication required)
        pub fn authenticate(&self, _username: &str, _password: &str) -> Result<Arc<AclUser>, ()> {
            Ok(Arc::new(AclUser::default_user()))
        }

        /// Always permits commands
        pub fn check_command(
            &self,
            _user: Option<&AclUser>,
            _command: &str,
            _keys: &[&str],
        ) -> Result<(), ()> {
            Ok(())
        }

        /// Returns whether authentication is required (always false in noop)
        pub fn requires_auth(&self) -> bool {
            false
        }
    }
}

#[cfg(not(feature = "acl"))]
pub use acl_noop::{AclManager, AclUser};
