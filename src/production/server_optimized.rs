use super::connection_optimized::{ConnectionConfig, OptimizedConnectionHandler};
use super::ttl_manager::TtlManagerActor;
use super::{ConnectionPool, PerformanceConfig, ServerConfig, ShardedActorState};
use crate::observability::{DatadogConfig, Metrics};
use crate::security::AclManager;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info, warn};

#[cfg(feature = "tls")]
use crate::security::tls::{MaybeSecureStream, TlsAcceptor};

pub struct OptimizedRedisServer {
    addr: String,
}

impl OptimizedRedisServer {
    #[inline]
    pub fn new(addr: String) -> Self {
        debug_assert!(!addr.is_empty(), "Server address cannot be empty");
        OptimizedRedisServer { addr }
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Load performance configuration from file or environment
        let perf_config = PerformanceConfig::from_env();
        if let Err(e) = perf_config.validate() {
            error!("Invalid performance config: {}", e);
            return Err(e.into());
        }

        info!(
            "Performance config: shards={}, pool_capacity={}, pool_prewarm={}, read_buffer={}, min_pipeline={}",
            perf_config.num_shards,
            perf_config.response_pool.capacity,
            perf_config.response_pool.prewarm,
            perf_config.buffers.read_size,
            perf_config.batching.min_pipeline_buffer,
        );

        // Load security configuration
        let server_config = ServerConfig::from_env();

        // Build TLS acceptor if TLS is configured
        #[cfg(feature = "tls")]
        let tls_acceptor: Option<TlsAcceptor> = if let Some(tls_config) = &server_config.tls {
            match tls_config.build_acceptor() {
                Ok(acceptor) => {
                    info!(
                        "TLS enabled with cert={:?}, key={:?}",
                        tls_config.cert_path, tls_config.key_path
                    );
                    if tls_config.require_client_cert {
                        info!("Mutual TLS (mTLS) enabled - client certificates required");
                    }
                    Some(acceptor)
                }
                Err(e) => {
                    error!("Failed to initialize TLS: {}", e);
                    return Err(Box::new(e));
                }
            }
        } else {
            info!("TLS disabled (set TLS_CERT_PATH and TLS_KEY_PATH to enable)");
            None
        };

        #[cfg(not(feature = "tls"))]
        if server_config.tls.is_some() {
            warn!("TLS configuration provided but 'tls' feature not enabled. Ignoring TLS settings.");
        }

        // Initialize ACL manager
        let acl_manager = Self::create_acl_manager(&server_config);
        let acl_manager = Arc::new(RwLock::new(acl_manager));

        let state = ShardedActorState::with_perf_config(&perf_config);
        let connection_pool = Arc::new(ConnectionPool::new(10000, 512));

        // Create connection config from performance config
        let conn_config =
            ConnectionConfig::from_perf_config(&perf_config.buffers, &perf_config.batching);

        // Initialize metrics
        let dd_config = DatadogConfig::from_env();
        let metrics = Arc::new(Metrics::new(&dd_config));

        info!(
            "Initialized Tiger Style Redis with {} shards (lock-free)",
            perf_config.num_shards
        );

        // Spawn TTL manager actor with shutdown handle
        let _ttl_handle = TtlManagerActor::spawn(state.clone(), metrics.clone());
        info!("TTL manager started (100ms interval)");

        let listener = TcpListener::bind(&self.addr).await?;
        info!("Redis server listening on {}", self.addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let client_addr = addr.to_string();
                    let state_clone = state.clone();
                    let pool = connection_pool.clone();
                    let metrics_clone = metrics.clone();
                    let conn_config_clone = conn_config.clone();
                    let acl_manager_clone = acl_manager.clone();

                    // Set TCP_NODELAY for lower latency before any wrapping
                    if let Err(e) = stream.set_nodelay(true) {
                        warn!("Failed to set TCP_NODELAY for {}: {}", client_addr, e);
                    }

                    #[cfg(feature = "tls")]
                    let tls_acceptor_clone = tls_acceptor.clone();

                    tokio::spawn(async move {
                        // TigerStyle: Handle Result instead of unwrap
                        let _permit = match pool.acquire_permit().await {
                            Ok(permit) => permit,
                            Err(e) => {
                                warn!("Failed to acquire connection permit: {}", e);
                                return;
                            }
                        };

                        // Wrap stream with TLS if enabled
                        #[cfg(feature = "tls")]
                        let stream = if let Some(acceptor) = tls_acceptor_clone {
                            match acceptor.accept(stream).await {
                                Ok(tls_stream) => MaybeSecureStream::tls(tls_stream),
                                Err(e) => {
                                    warn!("TLS handshake failed for {}: {}", client_addr, e);
                                    return;
                                }
                            }
                        } else {
                            MaybeSecureStream::plain(stream)
                        };

                        let handler = OptimizedConnectionHandler::new(
                            stream,
                            state_clone,
                            client_addr,
                            pool.buffer_pool(),
                            metrics_clone,
                            conn_config_clone,
                            acl_manager_clone,
                        );
                        handler.run().await;
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Create and configure ACL manager based on server configuration
    fn create_acl_manager(config: &ServerConfig) -> AclManager {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclUser;

            let mut manager = if config.acl.require_auth {
                AclManager::new_with_auth()
            } else {
                AclManager::new()
            };

            // Configure default user with password if REDIS_REQUIRE_PASS is set
            if let Some(ref password) = config.acl.require_pass {
                let mut default_user = AclUser::default_user();
                default_user.add_password(password);
                default_user.nopass = false; // Require password
                manager.set_user(default_user);
                info!("Authentication enabled (REDIS_REQUIRE_PASS set)");
            }

            // TODO: Load ACL file if configured
            if let Some(ref acl_file) = config.acl.acl_file {
                warn!("ACL file loading not yet implemented: {:?}", acl_file);
            }

            if !config.acl.require_auth {
                info!("ACL authentication disabled (set REDIS_REQUIRE_PASS to enable)");
            }

            manager
        }

        #[cfg(not(feature = "acl"))]
        {
            let _ = config;
            // Return no-op ACL manager when feature disabled
            AclManager::new()
        }
    }
}
