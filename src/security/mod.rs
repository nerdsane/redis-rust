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

    /// No-op ACL error - implements Display for compatibility
    #[derive(Debug, Clone)]
    pub struct AclError(String);

    impl std::fmt::Display for AclError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for AclError {}

    /// No-op ACL user - always permitted
    #[derive(Debug, Clone)]
    pub struct AclUser {
        pub name: String,
        pub enabled: bool,
    }

    impl AclUser {
        pub fn default_user() -> Self {
            Self {
                name: "default".to_string(),
                enabled: true,
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

        /// No-op: auth never required when ACL feature disabled
        pub fn new_with_auth() -> Self {
            Self
        }

        /// Get the default user
        pub fn default_user(&self) -> Arc<AclUser> {
            Arc::new(AclUser::default_user())
        }

        /// Always returns the default user (no authentication required)
        pub fn authenticate(&self, _username: &str, _password: &str) -> Result<Arc<AclUser>, AclError> {
            Ok(Arc::new(AclUser::default_user()))
        }

        /// Always permits commands
        pub fn check_command(
            &self,
            _user: Option<&AclUser>,
            _command: &str,
            _keys: &[&str],
        ) -> Result<(), AclError> {
            Ok(())
        }

        /// Returns whether authentication is required (always false in noop)
        pub fn requires_auth(&self) -> bool {
            false
        }

        /// No-op: set user does nothing when ACL feature disabled
        pub fn set_user(&mut self, _user: AclUser) {}

        /// No-op: get user returns None when ACL feature disabled
        pub fn get_user(&self, _username: &str) -> Option<Arc<AclUser>> {
            None
        }
    }
}

#[cfg(not(feature = "acl"))]
pub use acl_noop::{AclError, AclManager, AclUser};
