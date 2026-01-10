use super::connection_pool::BufferPoolAsync;
use super::perf_config::{BatchingConfig, BufferConfig};
use super::ShardedActorState;
use crate::observability::{spans, Metrics};
use crate::redis::{Command, RespCodec, RespValue};
use crate::security::{AclManager, AclUser};
use bytes::{BufMut, BytesMut};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::{debug, error, info, warn, Instrument};

// P3 optimization: Use itoa for fast integer encoding
#[cfg(feature = "opt-itoa-encode")]
use itoa;

// P5 optimization: Use atoi for fast integer parsing
#[cfg(feature = "opt-atoi-parse")]
use atoi;

/// P5 helper: Parse usize from bytes without UTF-8 validation overhead
#[cfg(feature = "opt-atoi-parse")]
#[inline]
fn parse_usize_fast(bytes: &[u8]) -> Option<usize> {
    atoi::atoi::<usize>(bytes)
}

/// P5 fallback: Parse usize via UTF-8 string conversion
#[cfg(not(feature = "opt-atoi-parse"))]
#[inline]
fn parse_usize_fast(bytes: &[u8]) -> Option<usize> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

/// Connection configuration (from PerformanceConfig)
#[derive(Clone)]
pub struct ConnectionConfig {
    pub max_buffer_size: usize,
    pub read_buffer_size: usize,
    pub min_pipeline_buffer: usize,
    pub batch_threshold: usize,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 1024 * 1024, // 1MB
            read_buffer_size: 8192,
            min_pipeline_buffer: 60,
            batch_threshold: 2,
        }
    }
}

impl ConnectionConfig {
    /// Create from PerformanceConfig components
    pub fn from_perf_config(buffers: &BufferConfig, batching: &BatchingConfig) -> Self {
        Self {
            max_buffer_size: buffers.max_size,
            read_buffer_size: buffers.read_size,
            min_pipeline_buffer: batching.min_pipeline_buffer,
            batch_threshold: batching.batch_threshold,
        }
    }
}

pub struct OptimizedConnectionHandler<S> {
    stream: S,
    state: ShardedActorState,
    buffer: BytesMut,
    write_buffer: BytesMut,
    client_addr: String,
    buffer_pool: Arc<BufferPoolAsync>,
    metrics: Arc<Metrics>,
    config: ConnectionConfig,
    /// ACL manager for authentication and authorization
    acl_manager: Arc<RwLock<AclManager>>,
    /// Currently authenticated user (None = not authenticated yet)
    authenticated_user: Option<Arc<AclUser>>,
}

impl<S> OptimizedConnectionHandler<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    #[inline]
    pub fn new(
        stream: S,
        state: ShardedActorState,
        client_addr: String,
        buffer_pool: Arc<BufferPoolAsync>,
        metrics: Arc<Metrics>,
        config: ConnectionConfig,
        acl_manager: Arc<RwLock<AclManager>>,
        client_cert_cn: Option<String>,
    ) -> Self {
        let buffer = buffer_pool.acquire();
        let write_buffer = buffer_pool.acquire();
        debug_assert!(
            buffer.capacity() > 0,
            "Buffer pool returned zero-capacity buffer"
        );
        debug_assert!(
            config.max_buffer_size >= config.read_buffer_size,
            "max_buffer_size must be >= read_buffer_size"
        );

        // Auto-authenticate based on priority:
        // 1. Client certificate CN (if provided and matches an ACL user)
        // 2. Default user (if ACL doesn't require auth)
        let authenticated_user = {
            let manager = acl_manager.read();

            // Try client certificate authentication first
            if let Some(ref cn) = client_cert_cn {
                if let Some(user) = manager.get_user(cn) {
                    if user.enabled {
                        info!(
                            "Client {} authenticated via certificate as '{}'",
                            client_addr, cn
                        );
                        Some(user)
                    } else {
                        warn!(
                            "Client {} has certificate for disabled user '{}'",
                            client_addr, cn
                        );
                        None
                    }
                } else {
                    warn!(
                        "Client {} has certificate CN '{}' but no matching ACL user",
                        client_addr, cn
                    );
                    // Fall through to default auth
                    if !manager.requires_auth() {
                        Some(manager.default_user())
                    } else {
                        None
                    }
                }
            } else if !manager.requires_auth() {
                Some(manager.default_user())
            } else {
                None
            }
        };

        OptimizedConnectionHandler {
            stream,
            state,
            buffer,
            write_buffer,
            client_addr,
            buffer_pool,
            metrics,
            config,
            acl_manager,
            authenticated_user,
        }
    }

    pub async fn run(mut self) {
        // Create connection span for distributed tracing
        let connection_span = spans::connection_span(&self.client_addr);

        async {
            info!("Client connected: {}", self.client_addr);
            self.metrics.record_connection("established");

            // Note: TCP_NODELAY should be set at the server level before passing the stream

            // Use config for read buffer size (stack-allocate with max expected size)
            let mut read_buf = vec![0u8; self.config.read_buffer_size];

            loop {
                match self.stream.read(&mut read_buf).await {
                    Ok(0) => {
                        info!("Client disconnected: {}", self.client_addr);
                        break;
                    }
                    Ok(n) => {
                        if self.buffer.len() + n > self.config.max_buffer_size {
                            error!(
                                "Buffer overflow from {}, closing connection",
                                self.client_addr
                            );
                            Self::encode_error_into("buffer overflow", &mut self.write_buffer);
                            let _ = self.stream.write_all(&self.write_buffer).await;
                            break;
                        }

                        self.buffer.extend_from_slice(&read_buf[..n]);

                        // Process ALL available commands (pipelining support)
                        let mut commands_executed = 0;
                        let mut had_parse_error = false;

                        // OPTIMIZATION: Only attempt batching when buffer is large enough
                        // to contain multiple commands. A single GET/SET is ~25-50 bytes, so
                        // 60+ bytes likely means pipelined commands.
                        // This avoids parsing overhead for P=1 (single command) scenarios.
                        let min_pipeline_buffer = self.config.min_pipeline_buffer;
                        let batch_threshold = self.config.batch_threshold;

                        if self.buffer.len() >= min_pipeline_buffer {
                            // Try GET batching first
                            let (get_keys, get_count) = self.collect_get_keys();

                            if get_count >= batch_threshold {
                                // Batch execute multiple GETs concurrently
                                let start = Instant::now();
                                let results = self.state.fast_batch_get_pipeline(get_keys).await;
                                let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

                                for response in &results {
                                    let success = !matches!(response, RespValue::Error(_));
                                    self.metrics.record_command(
                                        "GET",
                                        duration_ms / results.len() as f64,
                                        success,
                                    );
                                    Self::encode_resp_into(response, &mut self.write_buffer);
                                }
                                commands_executed += get_count;
                            }

                            // Try SET batching if buffer still has enough data
                            if self.buffer.len() >= min_pipeline_buffer {
                                let (set_pairs, set_count) = self.collect_set_pairs();

                                if set_count >= batch_threshold {
                                    // Batch execute multiple SETs concurrently
                                    let start = Instant::now();
                                    let results =
                                        self.state.fast_batch_set_pipeline(set_pairs).await;
                                    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

                                    for response in &results {
                                        let success = !matches!(response, RespValue::Error(_));
                                        self.metrics.record_command(
                                            "SET",
                                            duration_ms / results.len() as f64,
                                            success,
                                        );
                                        Self::encode_resp_into(response, &mut self.write_buffer);
                                    }
                                    commands_executed += set_count;
                                }
                            }
                        }

                        // Process remaining commands sequentially
                        loop {
                            match self.try_execute_command().await {
                                CommandResult::Executed => {
                                    commands_executed += 1;
                                    // Don't flush yet - continue processing pipeline
                                }
                                CommandResult::NeedMoreData => break,
                                CommandResult::ParseError(e) => {
                                    warn!(
                                        "Parse error from {}: {}, draining buffer",
                                        self.client_addr, e
                                    );
                                    self.buffer.clear();
                                    Self::encode_error_into(
                                        "protocol error",
                                        &mut self.write_buffer,
                                    );
                                    had_parse_error = true;
                                    break;
                                }
                            }
                        }

                        // Flush ALL responses at once (critical for pipelining performance)
                        if !self.write_buffer.is_empty() {
                            if let Err(e) = self.stream.write_all(&self.write_buffer).await {
                                error!("Write failed to {}: {}", self.client_addr, e);
                                break;
                            }
                            // Ensure data is sent immediately
                            if let Err(e) = self.stream.flush().await {
                                error!("Flush failed to {}: {}", self.client_addr, e);
                                break;
                            }
                            self.write_buffer.clear();
                        }

                        if had_parse_error {
                            // Continue to next read after parse error
                        }

                        debug!("Processed {} commands in pipeline batch", commands_executed);
                    }
                    Err(e) => {
                        debug!("Read error from {}: {}", self.client_addr, e);
                        break;
                    }
                }
            }

            self.metrics.record_connection("closed");
            self.buffer_pool.release(self.buffer);
            self.buffer_pool.release(self.write_buffer);
        }
        .instrument(connection_span)
        .await
    }

    #[inline]
    async fn try_execute_command(&mut self) -> CommandResult {
        // Try fast path first for GET/SET commands (80%+ of traffic)
        // Fast path skips ACL checks for performance - only use when auth not required
        if self.authenticated_user.is_some() {
            match self.try_fast_path().await {
                FastPathResult::Handled => return CommandResult::Executed,
                FastPathResult::NeedMoreData => return CommandResult::NeedMoreData,
                FastPathResult::NotFastPath => {} // Fall through to regular parsing
            }
        }

        match RespCodec::parse(&mut self.buffer) {
            Ok(Some(resp_value)) => match Command::from_resp_zero_copy(&resp_value) {
                Ok(cmd) => {
                    let cmd_name = cmd.name();
                    let start = Instant::now();

                    // Handle AUTH and ACL commands specially
                    let response = match &cmd {
                        Command::Auth { username, password } => {
                            self.handle_auth(username.as_deref(), password)
                        }
                        Command::AclWhoami => self.handle_acl_whoami(),
                        Command::AclList => self.handle_acl_list(),
                        Command::AclUsers => self.handle_acl_users(),
                        Command::AclGetUser { username } => self.handle_acl_getuser(username),
                        Command::AclSetUser { username, rules } => {
                            self.handle_acl_setuser(username, rules)
                        }
                        Command::AclDelUser { usernames } => self.handle_acl_deluser(usernames),
                        Command::AclCat { category } => self.handle_acl_cat(category.as_deref()),
                        Command::AclGenPass { bits } => self.handle_acl_genpass(*bits),
                        _ => {
                            // Check ACL permissions for regular commands
                            if let Err(acl_err) = self.check_acl_permission(&cmd) {
                                RespValue::Error(acl_err)
                            } else {
                                self.state.execute(&cmd).await
                            }
                        }
                    };

                    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
                    let success = !matches!(&response, RespValue::Error(_));
                    self.metrics.record_command(cmd_name, duration_ms, success);

                    Self::encode_resp_into(&response, &mut self.write_buffer);
                    CommandResult::Executed
                }
                Err(e) => {
                    self.metrics.record_command("PARSE_ERROR", 0.0, false);
                    Self::encode_error_into(&e, &mut self.write_buffer);
                    CommandResult::Executed
                }
            },
            Ok(None) => CommandResult::NeedMoreData,
            Err(e) => CommandResult::ParseError(e),
        }
    }

    /// Check ACL permissions for a command
    fn check_acl_permission(&self, cmd: &Command) -> Result<(), String> {
        let manager = self.acl_manager.read();

        // If auth is required but user not authenticated, reject
        if manager.requires_auth() && self.authenticated_user.is_none() {
            return Err("NOAUTH Authentication required".to_string());
        }

        // Get the authenticated user (if any)
        let user = self.authenticated_user.as_ref().map(|u| u.as_ref());

        // Get the keys involved in this command
        let owned_keys = cmd.get_keys();
        let keys: Vec<&str> = owned_keys.iter().map(|s| s.as_str()).collect();

        // Check permissions
        manager
            .check_command(user, cmd.name(), &keys)
            .map_err(|e| e.to_string())
    }

    /// Handle AUTH command
    fn handle_auth(&mut self, username: Option<&str>, password: &str) -> RespValue {
        let username = username.unwrap_or("default");
        let manager = self.acl_manager.read();

        match manager.authenticate(username, password) {
            Ok(user) => {
                drop(manager); // Release read lock before mutating self
                self.authenticated_user = Some(user);
                info!("Client {} authenticated as '{}'", self.client_addr, username);
                RespValue::SimpleString("OK".to_string())
            }
            Err(e) => {
                warn!(
                    "Auth failed for client {} (user '{}'): {}",
                    self.client_addr, username, e
                );
                RespValue::Error(e.to_string())
            }
        }
    }

    /// Handle ACL WHOAMI command
    fn handle_acl_whoami(&self) -> RespValue {
        let name = match &self.authenticated_user {
            Some(user) => user.name.clone(),
            None => "default".to_string(),
        };
        RespValue::BulkString(Some(name.into_bytes()))
    }

    /// Handle ACL LIST command
    fn handle_acl_list(&self) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclCommandHandler;
            let manager = self.acl_manager.read();
            let list = AclCommandHandler::handle_list(&manager);
            RespValue::Array(Some(
                list.into_iter()
                    .map(|s| RespValue::BulkString(Some(s.into_bytes())))
                    .collect(),
            ))
        }
        #[cfg(not(feature = "acl"))]
        {
            // Return minimal list when ACL feature disabled
            RespValue::Array(Some(vec![RespValue::BulkString(Some(
                "user default on nopass ~* +@all".as_bytes().to_vec(),
            ))]))
        }
    }

    /// Handle ACL USERS command
    fn handle_acl_users(&self) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclCommandHandler;
            let manager = self.acl_manager.read();
            let users = AclCommandHandler::handle_users(&manager);
            RespValue::Array(Some(
                users
                    .into_iter()
                    .map(|s| RespValue::BulkString(Some(s.into_bytes())))
                    .collect(),
            ))
        }
        #[cfg(not(feature = "acl"))]
        {
            RespValue::Array(Some(vec![RespValue::BulkString(Some(
                "default".as_bytes().to_vec(),
            ))]))
        }
    }

    /// Handle ACL GETUSER command
    fn handle_acl_getuser(&self, username: &str) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclCommandHandler;
            let manager = self.acl_manager.read();
            match AclCommandHandler::handle_getuser(&manager, username) {
                Some(info) => {
                    let mut result = Vec::new();
                    for (key, value) in info {
                        result.push(RespValue::BulkString(Some(key.into_bytes())));
                        result.push(RespValue::BulkString(Some(value.into_bytes())));
                    }
                    RespValue::Array(Some(result))
                }
                None => RespValue::Array(None), // Null array for non-existent user
            }
        }
        #[cfg(not(feature = "acl"))]
        {
            if username == "default" {
                RespValue::Array(Some(vec![
                    RespValue::BulkString(Some("flags".as_bytes().to_vec())),
                    RespValue::BulkString(Some("on nopass".as_bytes().to_vec())),
                ]))
            } else {
                RespValue::Array(None)
            }
        }
    }

    /// Handle ACL SETUSER command
    fn handle_acl_setuser(&mut self, username: &str, rules: &[String]) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclCommandHandler;
            let mut manager = self.acl_manager.write();
            let rule_refs: Vec<&str> = rules.iter().map(|s| s.as_str()).collect();
            match AclCommandHandler::handle_setuser(&mut manager, username, &rule_refs) {
                Ok(()) => RespValue::SimpleString("OK".to_string()),
                Err(e) => RespValue::Error(e.to_string()),
            }
        }
        #[cfg(not(feature = "acl"))]
        {
            let _ = (username, rules);
            RespValue::Error("ERR ACL feature not enabled".to_string())
        }
    }

    /// Handle ACL DELUSER command
    fn handle_acl_deluser(&mut self, usernames: &[String]) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclCommandHandler;
            let mut manager = self.acl_manager.write();
            let username_refs: Vec<&str> = usernames.iter().map(|s| s.as_str()).collect();
            match AclCommandHandler::handle_deluser(&mut manager, &username_refs) {
                Ok(count) => RespValue::Integer(count as i64),
                Err(e) => RespValue::Error(e.to_string()),
            }
        }
        #[cfg(not(feature = "acl"))]
        {
            let _ = usernames;
            RespValue::Error("ERR ACL feature not enabled".to_string())
        }
    }

    /// Handle ACL CAT command
    fn handle_acl_cat(&self, category: Option<&str>) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclCommandHandler;
            match AclCommandHandler::handle_cat(category) {
                Ok(items) => RespValue::Array(Some(
                    items
                        .into_iter()
                        .map(|s| RespValue::BulkString(Some(s.into_bytes())))
                        .collect(),
                )),
                Err(e) => RespValue::Error(e.to_string()),
            }
        }
        #[cfg(not(feature = "acl"))]
        {
            let _ = category;
            // Return basic categories even without ACL feature
            let categories = vec![
                "read", "write", "admin", "dangerous", "keyspace", "string", "list", "set", "hash",
                "sortedset", "connection", "server",
            ];
            RespValue::Array(Some(
                categories
                    .into_iter()
                    .map(|s| RespValue::BulkString(Some(s.as_bytes().to_vec())))
                    .collect(),
            ))
        }
    }

    /// Handle ACL GENPASS command
    fn handle_acl_genpass(&self, bits: Option<u32>) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::AclCommandHandler;
            let password = AclCommandHandler::handle_genpass(bits);
            RespValue::BulkString(Some(password.into_bytes()))
        }
        #[cfg(not(feature = "acl"))]
        {
            // Simple fallback password generation
            use std::time::{SystemTime, UNIX_EPOCH};
            let bits = bits.unwrap_or(256).min(1024);
            let bytes = (bits as usize + 7) / 8;
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let mut result = String::with_capacity(bytes * 2);
            let mut state = seed;
            for _ in 0..bytes {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let byte = ((state >> 33) & 0xFF) as u8;
                result.push_str(&format!("{:02x}", byte));
            }
            RespValue::BulkString(Some(result.into_bytes()))
        }
    }

    /// Collect all parseable GET keys from the buffer for batched execution
    ///
    /// Returns (keys, count) - the keys to GET and how many commands were parsed.
    /// Consumes the GET commands from the buffer.
    #[inline]
    fn collect_get_keys(&mut self) -> (Vec<bytes::Bytes>, usize) {
        let mut keys = Vec::new();
        const HEADER_LEN: usize = 14; // "*2\r\n$3\r\nGET\r\n"

        loop {
            let buf = &self.buffer[..];

            // Need minimum bytes to detect GET
            if buf.len() < HEADER_LEN + 1 {
                break;
            }

            // Check for GET command
            if !buf.starts_with(b"*2\r\n$3\r\nGET\r\n") && !buf.starts_with(b"*2\r\n$3\r\nget\r\n")
            {
                break; // Not a GET, stop collecting
            }

            // Parse key length: $<len>\r\n
            let after_header = &buf[HEADER_LEN..];
            if after_header.is_empty() || after_header[0] != b'$' {
                break;
            }

            // Find \r\n after key length
            let Some(crlf_pos) = memchr::memchr(b'\r', &after_header[1..]) else {
                break; // Need more data
            };
            let len_end = crlf_pos + 1;

            // Parse key length
            let len_str = &after_header[1..len_end];
            // P5: Fast integer parsing
            let Ok(key_len) = parse_usize_fast(len_str).ok_or(()) else {
                break; // Invalid, stop
            };

            // Check we have complete key + trailing \r\n
            let key_start = HEADER_LEN + 1 + len_end + 1;
            let total_needed = key_start + key_len + 2;

            if buf.len() < total_needed {
                break; // Need more data
            }

            // Extract key
            let key = bytes::Bytes::copy_from_slice(&buf[key_start..key_start + key_len]);
            keys.push(key);

            // Consume this GET from buffer
            let _ = self.buffer.split_to(total_needed);
        }

        let count = keys.len();
        (keys, count)
    }

    /// Collect all parseable SET key-value pairs from the buffer for batched execution
    ///
    /// Returns (pairs, count) - the (key, value) pairs to SET and how many commands were parsed.
    /// Consumes the SET commands from the buffer.
    #[inline]
    fn collect_set_pairs(&mut self) -> (Vec<(bytes::Bytes, bytes::Bytes)>, usize) {
        let mut pairs = Vec::new();
        const HEADER_LEN: usize = 14; // "*3\r\n$3\r\nSET\r\n"

        loop {
            let buf = &self.buffer[..];

            // Need minimum bytes to detect SET
            if buf.len() < HEADER_LEN + 1 {
                break;
            }

            // Check for SET command
            if !buf.starts_with(b"*3\r\n$3\r\nSET\r\n") && !buf.starts_with(b"*3\r\n$3\r\nset\r\n")
            {
                break; // Not a SET, stop collecting
            }

            // Parse key length: $<len>\r\n
            let after_header = &buf[HEADER_LEN..];
            if after_header.is_empty() || after_header[0] != b'$' {
                break;
            }

            // Find \r\n after key length
            let Some(key_len_crlf) = memchr::memchr(b'\r', &after_header[1..]) else {
                break; // Need more data
            };

            // Parse key length
            let key_len_str = &after_header[1..key_len_crlf + 1];
            // P5: Fast integer parsing
            let Ok(key_len) = parse_usize_fast(key_len_str).ok_or(()) else {
                break; // Invalid, stop
            };

            // Calculate key position
            let key_start = HEADER_LEN + 1 + key_len_crlf + 2; // After $<keylen>\r\n
            let key_end = key_start + key_len;
            let val_len_start = key_end + 2; // After key\r\n

            if buf.len() < val_len_start + 1 {
                break; // Need more data
            }

            // Parse value length: $<len>\r\n
            if buf[val_len_start] != b'$' {
                break; // Invalid format
            }

            let after_key = &buf[val_len_start + 1..];
            let Some(val_len_crlf) = memchr::memchr(b'\r', after_key) else {
                break; // Need more data
            };

            let val_len_str = &after_key[..val_len_crlf];
            // P5: Fast integer parsing
            let Ok(val_len) = parse_usize_fast(val_len_str).ok_or(()) else {
                break; // Invalid
            };

            // Calculate value position and total length
            let val_start = val_len_start + 1 + val_len_crlf + 2; // After $<vallen>\r\n
            let total_needed = val_start + val_len + 2; // value + \r\n

            if buf.len() < total_needed {
                break; // Need more data
            }

            // Extract key and value
            let key = bytes::Bytes::copy_from_slice(&buf[key_start..key_end]);
            let value = bytes::Bytes::copy_from_slice(&buf[val_start..val_start + val_len]);
            pairs.push((key, value));

            // Consume this SET from buffer
            let _ = self.buffer.split_to(total_needed);
        }

        let count = pairs.len();
        (pairs, count)
    }

    /// Fast path for GET/SET commands - bypasses full RESP parsing
    ///
    /// RESP format for GET: *2\r\n$3\r\nGET\r\n$<keylen>\r\n<key>\r\n
    /// RESP format for SET: *3\r\n$3\r\nSET\r\n$<keylen>\r\n<key>\r\n$<vallen>\r\n<value>\r\n
    #[inline]
    async fn try_fast_path(&mut self) -> FastPathResult {
        let buf = &self.buffer[..];

        // Need at least "*2\r\n$3\r\nGET" (12 bytes) to detect GET
        if buf.len() < 12 {
            return FastPathResult::NotFastPath;
        }

        // Check for GET: *2\r\n$3\r\nGET\r\n
        if buf.starts_with(b"*2\r\n$3\r\nGET\r\n") || buf.starts_with(b"*2\r\n$3\r\nget\r\n") {
            return self.try_fast_get().await;
        }

        // Check for SET: *3\r\n$3\r\nSET\r\n
        if buf.starts_with(b"*3\r\n$3\r\nSET\r\n") || buf.starts_with(b"*3\r\n$3\r\nset\r\n") {
            return self.try_fast_set().await;
        }

        FastPathResult::NotFastPath
    }

    /// Parse and execute GET command via fast path
    #[inline]
    async fn try_fast_get(&mut self) -> FastPathResult {
        // Format: *2\r\n$3\r\nGET\r\n$<keylen>\r\n<key>\r\n
        // Header is 14 bytes: "*2\r\n$3\r\nGET\r\n"
        const HEADER_LEN: usize = 14;

        let buf = &self.buffer[..];
        if buf.len() < HEADER_LEN + 1 {
            return FastPathResult::NeedMoreData;
        }

        // Parse key length: $<len>\r\n
        let after_header = &buf[HEADER_LEN..];
        if after_header[0] != b'$' {
            return FastPathResult::NotFastPath; // Malformed, fall back
        }

        // Find \r\n after key length
        let Some(crlf_pos) = memchr::memchr(b'\r', &after_header[1..]) else {
            return FastPathResult::NeedMoreData;
        };
        let len_end = crlf_pos + 1; // Position of \r relative to after_header[1..]

        // Parse key length
        let len_str = &after_header[1..len_end];
        // P5: Fast integer parsing
        let Ok(key_len) = parse_usize_fast(len_str).ok_or(()) else {
            return FastPathResult::NotFastPath; // Invalid length
        };

        // Check we have complete key + trailing \r\n
        let key_start = HEADER_LEN + 1 + len_end + 1; // After $<len>\r\n
        let total_needed = key_start + key_len + 2; // key + \r\n

        if buf.len() < total_needed {
            return FastPathResult::NeedMoreData;
        }

        // Extract key as bytes::Bytes (zero-copy from buffer)
        let key = bytes::Bytes::copy_from_slice(&buf[key_start..key_start + key_len]);

        // Consume the parsed bytes from buffer
        let _ = self.buffer.split_to(total_needed);

        // Execute fast GET using pooled response slot (avoids oneshot allocation)
        let start = Instant::now();
        let response = self.state.pooled_fast_get(key).await;
        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
        let success = !matches!(&response, RespValue::Error(_));
        self.metrics.record_command("GET", duration_ms, success);

        Self::encode_resp_into(&response, &mut self.write_buffer);
        FastPathResult::Handled
    }

    /// Parse and execute SET command via fast path
    #[inline]
    async fn try_fast_set(&mut self) -> FastPathResult {
        // Format: *3\r\n$3\r\nSET\r\n$<keylen>\r\n<key>\r\n$<vallen>\r\n<value>\r\n
        // Header is 14 bytes: "*3\r\n$3\r\nSET\r\n"
        const HEADER_LEN: usize = 14;

        let buf = &self.buffer[..];
        if buf.len() < HEADER_LEN + 1 {
            return FastPathResult::NeedMoreData;
        }

        // Parse key length: $<len>\r\n
        let after_header = &buf[HEADER_LEN..];
        if after_header[0] != b'$' {
            return FastPathResult::NotFastPath;
        }

        let Some(key_len_crlf) = memchr::memchr(b'\r', &after_header[1..]) else {
            return FastPathResult::NeedMoreData;
        };

        let key_len_str = &after_header[1..key_len_crlf + 1];
        // P5: Fast integer parsing
        let Ok(key_len) = parse_usize_fast(key_len_str).ok_or(()) else {
            return FastPathResult::NotFastPath;
        };

        // Calculate key position
        let key_start = HEADER_LEN + 1 + key_len_crlf + 2; // After $<keylen>\r\n
        let key_end = key_start + key_len;
        let val_len_start = key_end + 2; // After key\r\n

        if buf.len() < val_len_start + 1 {
            return FastPathResult::NeedMoreData;
        }

        // Parse value length: $<len>\r\n
        if buf[val_len_start] != b'$' {
            return FastPathResult::NotFastPath;
        }

        let after_key = &buf[val_len_start + 1..];
        let Some(val_len_crlf) = memchr::memchr(b'\r', after_key) else {
            return FastPathResult::NeedMoreData;
        };

        let val_len_str = &after_key[..val_len_crlf];
        // P5: Fast integer parsing
        let Ok(val_len) = parse_usize_fast(val_len_str).ok_or(()) else {
            return FastPathResult::NotFastPath;
        };

        // Calculate value position and total length
        let val_start = val_len_start + 1 + val_len_crlf + 2; // After $<vallen>\r\n
        let total_needed = val_start + val_len + 2; // value + \r\n

        if buf.len() < total_needed {
            return FastPathResult::NeedMoreData;
        }

        // Extract key and value as bytes::Bytes
        let key = bytes::Bytes::copy_from_slice(&buf[key_start..key_end]);
        let value = bytes::Bytes::copy_from_slice(&buf[val_start..val_start + val_len]);

        // Consume the parsed bytes
        let _ = self.buffer.split_to(total_needed);

        // Execute fast SET using pooled response slot (avoids oneshot allocation)
        let start = Instant::now();
        let response = self.state.pooled_fast_set(key, value).await;
        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
        let success = !matches!(&response, RespValue::Error(_));
        self.metrics.record_command("SET", duration_ms, success);

        Self::encode_resp_into(&response, &mut self.write_buffer);
        FastPathResult::Handled
    }

    /// Encode RESP value into buffer
    /// P3 optimization: Use itoa for fast integer encoding when opt-itoa-encode is enabled
    #[inline]
    fn encode_resp_into(value: &RespValue, buf: &mut BytesMut) {
        match value {
            RespValue::SimpleString(s) => {
                buf.put_u8(b'+');
                buf.extend_from_slice(s.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            RespValue::Error(s) => {
                buf.put_u8(b'-');
                buf.extend_from_slice(s.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            RespValue::Integer(n) => {
                buf.put_u8(b':');
                // P3 optimization: Use itoa for fast integer formatting
                #[cfg(feature = "opt-itoa-encode")]
                {
                    let mut itoa_buf = itoa::Buffer::new();
                    let s = itoa_buf.format(*n);
                    buf.extend_from_slice(s.as_bytes());
                }
                #[cfg(not(feature = "opt-itoa-encode"))]
                {
                    buf.extend_from_slice(n.to_string().as_bytes());
                }
                buf.extend_from_slice(b"\r\n");
            }
            RespValue::BulkString(None) => {
                buf.extend_from_slice(b"$-1\r\n");
            }
            RespValue::BulkString(Some(data)) => {
                buf.put_u8(b'$');
                // P3 optimization: Use itoa for fast integer formatting
                #[cfg(feature = "opt-itoa-encode")]
                {
                    let mut itoa_buf = itoa::Buffer::new();
                    let s = itoa_buf.format(data.len());
                    buf.extend_from_slice(s.as_bytes());
                }
                #[cfg(not(feature = "opt-itoa-encode"))]
                {
                    buf.extend_from_slice(data.len().to_string().as_bytes());
                }
                buf.extend_from_slice(b"\r\n");
                buf.extend_from_slice(data);
                buf.extend_from_slice(b"\r\n");
            }
            RespValue::Array(None) => {
                buf.extend_from_slice(b"*-1\r\n");
            }
            RespValue::Array(Some(elements)) => {
                buf.put_u8(b'*');
                // P3 optimization: Use itoa for fast integer formatting
                #[cfg(feature = "opt-itoa-encode")]
                {
                    let mut itoa_buf = itoa::Buffer::new();
                    let s = itoa_buf.format(elements.len());
                    buf.extend_from_slice(s.as_bytes());
                }
                #[cfg(not(feature = "opt-itoa-encode"))]
                {
                    buf.extend_from_slice(elements.len().to_string().as_bytes());
                }
                buf.extend_from_slice(b"\r\n");
                for elem in elements {
                    Self::encode_resp_into(elem, buf);
                }
            }
        }
    }

    #[inline]
    fn encode_error_into(msg: &str, buf: &mut BytesMut) {
        buf.put_u8(b'-');
        buf.extend_from_slice(b"ERR ");
        buf.extend_from_slice(msg.as_bytes());
        buf.extend_from_slice(b"\r\n");
    }
}

enum CommandResult {
    Executed,
    NeedMoreData,
    ParseError(String),
}

/// Result of attempting fast path execution
enum FastPathResult {
    /// Command handled via fast path
    Handled,
    /// Need more data to complete parsing
    NeedMoreData,
    /// Not a fast-path command, fall back to regular parsing
    NotFastPath,
}
