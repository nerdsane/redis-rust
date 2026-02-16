//! TLS configuration and certificate loading

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls_pemfile::{certs, private_key};
use tokio_rustls::rustls::{
    pki_types::{CertificateDer, PrivateKeyDer},
    server::WebPkiClientVerifier,
    RootCertStore, ServerConfig,
};
use tokio_rustls::TlsAcceptor;

/// TLS-related errors
#[derive(Debug)]
pub enum TlsError {
    /// Failed to read certificate file
    CertificateReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Failed to read private key file
    PrivateKeyReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Failed to read CA certificate file
    CaReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    /// No certificates found in file
    NoCertificates { path: PathBuf },
    /// No private key found in file
    NoPrivateKey { path: PathBuf },
    /// Invalid certificate
    InvalidCertificate { reason: String },
    /// TLS configuration error
    ConfigError { reason: String },
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsError::CertificateReadError { path, source } => {
                write!(f, "Failed to read certificate from {:?}: {}", path, source)
            }
            TlsError::PrivateKeyReadError { path, source } => {
                write!(f, "Failed to read private key from {:?}: {}", path, source)
            }
            TlsError::CaReadError { path, source } => {
                write!(
                    f,
                    "Failed to read CA certificate from {:?}: {}",
                    path, source
                )
            }
            TlsError::NoCertificates { path } => {
                write!(f, "No certificates found in {:?}", path)
            }
            TlsError::NoPrivateKey { path } => {
                write!(f, "No private key found in {:?}", path)
            }
            TlsError::InvalidCertificate { reason } => {
                write!(f, "Invalid certificate: {}", reason)
            }
            TlsError::ConfigError { reason } => {
                write!(f, "TLS configuration error: {}", reason)
            }
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsError::CertificateReadError { source, .. } => Some(source),
            TlsError::PrivateKeyReadError { source, .. } => Some(source),
            TlsError::CaReadError { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// TLS configuration
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to server certificate file (PEM format)
    pub cert_path: PathBuf,
    /// Path to server private key file (PEM format)
    pub key_path: PathBuf,
    /// Path to CA certificate for client verification (optional)
    pub ca_path: Option<PathBuf>,
    /// Whether to require client certificates (mutual TLS)
    pub require_client_cert: bool,
}

impl TlsConfig {
    /// Create a new TLS configuration
    pub fn new(cert_path: impl AsRef<Path>, key_path: impl AsRef<Path>) -> Self {
        Self {
            cert_path: cert_path.as_ref().to_path_buf(),
            key_path: key_path.as_ref().to_path_buf(),
            ca_path: None,
            require_client_cert: false,
        }
    }

    /// Set CA certificate path for client verification
    pub fn with_ca(mut self, ca_path: impl AsRef<Path>) -> Self {
        self.ca_path = Some(ca_path.as_ref().to_path_buf());
        self
    }

    /// Require client certificates (mutual TLS)
    pub fn require_client_cert(mut self, require: bool) -> Self {
        self.require_client_cert = require;
        self
    }

    /// Load certificates from file
    fn load_certs(&self) -> Result<Vec<CertificateDer<'static>>, TlsError> {
        let file = File::open(&self.cert_path).map_err(|e| TlsError::CertificateReadError {
            path: self.cert_path.clone(),
            source: e,
        })?;
        let mut reader = BufReader::new(file);
        let certs: Vec<_> = certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TlsError::CertificateReadError {
                path: self.cert_path.clone(),
                source: e,
            })?;

        if certs.is_empty() {
            return Err(TlsError::NoCertificates {
                path: self.cert_path.clone(),
            });
        }

        Ok(certs)
    }

    /// Load private key from file
    fn load_private_key(&self) -> Result<PrivateKeyDer<'static>, TlsError> {
        let file = File::open(&self.key_path).map_err(|e| TlsError::PrivateKeyReadError {
            path: self.key_path.clone(),
            source: e,
        })?;
        let mut reader = BufReader::new(file);

        private_key(&mut reader)
            .map_err(|e| TlsError::PrivateKeyReadError {
                path: self.key_path.clone(),
                source: e,
            })?
            .ok_or_else(|| TlsError::NoPrivateKey {
                path: self.key_path.clone(),
            })
    }

    /// Load CA certificates for client verification
    fn load_ca_certs(&self) -> Result<Option<RootCertStore>, TlsError> {
        let ca_path = match &self.ca_path {
            Some(p) => p,
            None => return Ok(None),
        };

        let file = File::open(ca_path).map_err(|e| TlsError::CaReadError {
            path: ca_path.clone(),
            source: e,
        })?;
        let mut reader = BufReader::new(file);

        let certs: Vec<_> = certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TlsError::CaReadError {
                path: ca_path.clone(),
                source: e,
            })?;

        let mut root_store = RootCertStore::empty();
        for cert in certs {
            root_store
                .add(cert)
                .map_err(|e| TlsError::InvalidCertificate {
                    reason: e.to_string(),
                })?;
        }

        Ok(Some(root_store))
    }

    /// Build a TLS acceptor from this configuration
    pub fn build_acceptor(&self) -> Result<TlsAcceptor, TlsError> {
        let certs = self.load_certs()?;
        let key = self.load_private_key()?;

        let config = if let Some(root_store) = self.load_ca_certs()? {
            // Client certificate verification enabled
            let client_verifier = if self.require_client_cert {
                WebPkiClientVerifier::builder(Arc::new(root_store))
                    .build()
                    .map_err(|e| TlsError::ConfigError {
                        reason: e.to_string(),
                    })?
            } else {
                WebPkiClientVerifier::builder(Arc::new(root_store))
                    .allow_unauthenticated()
                    .build()
                    .map_err(|e| TlsError::ConfigError {
                        reason: e.to_string(),
                    })?
            };

            ServerConfig::builder()
                .with_client_cert_verifier(client_verifier)
                .with_single_cert(certs, key)
                .map_err(|e| TlsError::ConfigError {
                    reason: e.to_string(),
                })?
        } else {
            // No client verification
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|e| TlsError::ConfigError {
                    reason: e.to_string(),
                })?
        };

        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config = TlsConfig::new("/path/to/cert.pem", "/path/to/key.pem")
            .with_ca("/path/to/ca.pem")
            .require_client_cert(true);

        assert_eq!(config.cert_path, PathBuf::from("/path/to/cert.pem"));
        assert_eq!(config.key_path, PathBuf::from("/path/to/key.pem"));
        assert_eq!(config.ca_path, Some(PathBuf::from("/path/to/ca.pem")));
        assert!(config.require_client_cert);
    }

    // Note: Integration tests with actual certificates would go in tests/tls_test.rs
}
