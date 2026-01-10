//! Server configuration for TLS and ACL
//!
//! Configuration is loaded from environment variables:
//!
//! ## TLS Configuration (requires `tls` feature)
//! - `TLS_CERT_PATH`: Path to server certificate (PEM)
//! - `TLS_KEY_PATH`: Path to server private key (PEM)
//! - `TLS_CA_PATH`: Path to CA certificate for client verification (optional)
//! - `TLS_REQUIRE_CLIENT_CERT`: Require client certificates (default: false)
//!
//! ## ACL Configuration (requires `acl` feature)
//! - `REDIS_REQUIRE_PASS`: Simple password for AUTH (optional)
//! - `ACL_FILE`: Path to ACL configuration file (optional)

use std::path::PathBuf;

/// Server security configuration
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    /// TLS configuration (None = TLS disabled)
    pub tls: Option<TlsServerConfig>,
    /// ACL configuration
    pub acl: AclServerConfig,
}

/// TLS server configuration
#[derive(Debug, Clone)]
pub struct TlsServerConfig {
    /// Path to server certificate file (PEM)
    pub cert_path: PathBuf,
    /// Path to server private key file (PEM)
    pub key_path: PathBuf,
    /// Path to CA certificate for client verification (optional)
    pub ca_path: Option<PathBuf>,
    /// Whether to require client certificates
    pub require_client_cert: bool,
}

/// ACL server configuration
#[derive(Debug, Clone, Default)]
pub struct AclServerConfig {
    /// Simple password for AUTH command (Redis requirepass equivalent)
    pub require_pass: Option<String>,
    /// Path to ACL file (Redis aclfile equivalent)
    pub acl_file: Option<PathBuf>,
    /// Whether to require authentication
    pub require_auth: bool,
}

impl ServerConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let tls = Self::load_tls_config();
        let acl = Self::load_acl_config();

        Self { tls, acl }
    }

    fn load_tls_config() -> Option<TlsServerConfig> {
        let cert_path = std::env::var("TLS_CERT_PATH").ok()?;
        let key_path = std::env::var("TLS_KEY_PATH").ok()?;

        Some(TlsServerConfig {
            cert_path: PathBuf::from(cert_path),
            key_path: PathBuf::from(key_path),
            ca_path: std::env::var("TLS_CA_PATH").ok().map(PathBuf::from),
            require_client_cert: std::env::var("TLS_REQUIRE_CLIENT_CERT")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
        })
    }

    fn load_acl_config() -> AclServerConfig {
        let require_pass = std::env::var("REDIS_REQUIRE_PASS").ok();
        let acl_file = std::env::var("ACL_FILE").ok().map(PathBuf::from);
        let require_auth = require_pass.is_some() || acl_file.is_some();

        AclServerConfig {
            require_pass,
            acl_file,
            require_auth,
        }
    }

    /// Check if TLS is enabled
    pub fn tls_enabled(&self) -> bool {
        self.tls.is_some()
    }

    /// Check if ACL/authentication is enabled
    pub fn acl_enabled(&self) -> bool {
        self.acl.require_auth
    }
}

impl TlsServerConfig {
    /// Build a TLS acceptor from this configuration
    #[cfg(feature = "tls")]
    pub fn build_acceptor(&self) -> Result<crate::security::tls::TlsAcceptor, crate::security::tls::TlsError> {
        use crate::security::tls::TlsConfig;

        let mut config = TlsConfig::new(&self.cert_path, &self.key_path);

        if let Some(ca_path) = &self.ca_path {
            config = config.with_ca(ca_path);
        }

        if self.require_client_cert {
            config = config.require_client_cert(true);
        }

        config.build_acceptor()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert!(config.tls.is_none());
        assert!(!config.acl.require_auth);
    }

    #[test]
    fn test_tls_enabled() {
        let config = ServerConfig {
            tls: Some(TlsServerConfig {
                cert_path: PathBuf::from("/path/to/cert"),
                key_path: PathBuf::from("/path/to/key"),
                ca_path: None,
                require_client_cert: false,
            }),
            acl: AclServerConfig::default(),
        };
        assert!(config.tls_enabled());
    }
}
