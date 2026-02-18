//! Redis 6.0+ compatible ACL (Access Control List) system
//!
//! Provides user authentication and command authorization with:
//! - Multiple users with passwords (SHA256 hashed)
//! - Per-user command permissions (allow/deny lists, categories)
//! - Per-user key pattern restrictions
//! - Default user for backwards compatibility

mod commands;
mod file;
mod patterns;
mod user;

pub use commands::{apply_rule, AclCommandHandler};
pub use file::{load_acl_file, save_acl_file, AclFileError};
pub use patterns::{KeyPattern, KeyPatterns};
pub use user::{AclUser, CommandCategory, CommandPermissions};

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// ACL-related errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclError {
    /// Authentication failed (wrong username or password)
    AuthFailed,
    /// User is disabled
    UserDisabled,
    /// Command not permitted for this user
    CommandNotPermitted { command: String, user: String },
    /// Key access not permitted for this user
    KeyNotPermitted { key: String, user: String },
    /// User not found
    UserNotFound { username: String },
    /// User already exists
    UserAlreadyExists { username: String },
    /// Invalid ACL rule
    InvalidRule { rule: String, reason: String },
    /// Connection not authenticated
    NotAuthenticated,
}

impl std::fmt::Display for AclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AclError::AuthFailed => write!(
                f,
                "WRONGPASS invalid username-password pair or user is disabled."
            ),
            AclError::UserDisabled => write!(f, "NOPERM user is disabled"),
            AclError::CommandNotPermitted { command, user } => {
                write!(
                    f,
                    "NOPERM this user has no permissions to run the '{}' command",
                    command.to_lowercase()
                )
            }
            AclError::KeyNotPermitted { key, user } => {
                let _ = user;
                write!(
                    f,
                    "NOPERM this user has no permissions to access one of the keys used as arguments"
                )
            }
            AclError::UserNotFound { username } => {
                write!(f, "ERR User {} not found", username)
            }
            AclError::UserAlreadyExists { username } => {
                write!(f, "ERR User {} already exists", username)
            }
            AclError::InvalidRule { rule, reason } => {
                write!(f, "ERR Error in ACL SETUSER modifier '{}': {}", rule, reason)
            }
            AclError::NotAuthenticated => {
                write!(f, "NOAUTH Authentication required")
            }
        }
    }
}

impl std::error::Error for AclError {}

/// Reason for an ACL denial
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AclLogReason {
    /// Command not permitted
    Command,
    /// Key access denied
    Key,
    /// Channel access denied
    Channel,
    /// Authentication failure
    Auth,
}

impl AclLogReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            AclLogReason::Command => "command",
            AclLogReason::Key => "key",
            AclLogReason::Channel => "channel",
            AclLogReason::Auth => "auth",
        }
    }
}

impl std::fmt::Display for AclLogReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An entry in the ACL log
#[derive(Debug, Clone)]
pub struct AclLogEntry {
    /// Monotonic entry ID
    pub entry_id: u64,
    /// Number of times this denial was seen (aggregated)
    pub count: u64,
    /// Reason for denial
    pub reason: AclLogReason,
    /// Context (toplevel or multi)
    pub context: String,
    /// Object: the command name or key that was denied
    pub object: String,
    /// Username that was denied
    pub username: String,
    /// Client info string
    pub client_info: String,
    /// Timestamp when this entry was first created (epoch seconds with fractional)
    pub timestamp_created: f64,
    /// Timestamp when this entry was last updated (epoch seconds with fractional)
    pub timestamp_last_updated: f64,
}

/// Aggregation key for ACL log entries
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AclLogAggKey {
    username: String,
    reason: AclLogReason,
    object: String,
    context: String,
}

/// ACL log store with aggregation
#[derive(Debug)]
pub struct AclLogStore {
    /// All log entries in insertion order
    entries: Vec<AclLogEntry>,
    /// Aggregation index: key -> index in entries vec
    agg_index: HashMap<AclLogAggKey, usize>,
    /// Next monotonic entry ID
    next_entry_id: u64,
    /// Maximum log entries (Redis default: 128)
    max_entries: usize,
}

impl AclLogStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            agg_index: HashMap::new(),
            next_entry_id: 0,
            max_entries: 128,
        }
    }

    pub fn now_epoch_secs() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    /// Record an ACL denial
    pub fn record_denial(
        &mut self,
        username: &str,
        reason: AclLogReason,
        object: &str,
        context: &str,
        client_info: &str,
    ) {
        let now = Self::now_epoch_secs();
        let key = AclLogAggKey {
            username: username.to_string(),
            reason: reason.clone(),
            object: object.to_string(),
            context: context.to_string(),
        };

        if let Some(&idx) = self.agg_index.get(&key) {
            // Aggregate: increment count, update timestamp
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.count = entry.count.saturating_add(1);
                entry.timestamp_last_updated = now;
            }
        } else {
            // New entry
            let entry = AclLogEntry {
                entry_id: self.next_entry_id,
                count: 1,
                reason,
                context: context.to_string(),
                object: object.to_string(),
                username: username.to_string(),
                client_info: client_info.to_string(),
                timestamp_created: now,
                timestamp_last_updated: now,
            };
            self.next_entry_id = self.next_entry_id.saturating_add(1);

            // Evict oldest if at capacity
            if self.entries.len() >= self.max_entries {
                // Remove the oldest entry and its index
                let removed = self.entries.remove(0);
                let removed_key = AclLogAggKey {
                    username: removed.username,
                    reason: removed.reason,
                    object: removed.object,
                    context: removed.context,
                };
                self.agg_index.remove(&removed_key);
                // Update all indices (shifted down by 1)
                for val in self.agg_index.values_mut() {
                    *val = val.saturating_sub(1);
                }
            }

            let idx = self.entries.len();
            self.agg_index.insert(key, idx);
            self.entries.push(entry);
        }
    }

    /// Get log entries (most recent first). If count is None, return all.
    pub fn get_log(&self, count: Option<usize>) -> Vec<&AclLogEntry> {
        let entries: Vec<&AclLogEntry> = self.entries.iter().rev().collect();
        match count {
            Some(n) => entries.into_iter().take(n).collect(),
            None => entries,
        }
    }

    /// Reset (clear) the log
    pub fn reset(&mut self) {
        self.entries.clear();
        self.agg_index.clear();
        // Don't reset next_entry_id — keep monotonic
    }
}

impl Default for AclLogStore {
    fn default() -> Self {
        Self::new()
    }
}

/// ACL Manager - manages users and authorization
#[derive(Debug)]
pub struct AclManager {
    /// All registered users
    users: HashMap<String, Arc<AclUser>>,
    /// Whether authentication is required for new connections
    require_auth: bool,
    /// ACL denial log
    pub acl_log: AclLogStore,
}

impl AclManager {
    /// Create a new ACL manager with default user
    pub fn new() -> Self {
        let mut users = HashMap::new();
        let default_user = AclUser::default_user();
        users.insert("default".to_string(), Arc::new(default_user));

        Self {
            users,
            require_auth: false,
            acl_log: AclLogStore::new(),
        }
    }

    /// Create an ACL manager that requires authentication
    pub fn new_with_auth() -> Self {
        let mut manager = Self::new();
        manager.require_auth = true;
        manager
    }

    /// Hash a password using SHA256 (Redis uses SHA256 for ACL passwords)
    pub fn hash_password(password: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Check if authentication is required
    pub fn requires_auth(&self) -> bool {
        self.require_auth
    }

    /// Set whether authentication is required
    pub fn set_require_auth(&mut self, require: bool) {
        self.require_auth = require;
    }

    /// Authenticate a user with username and password
    pub fn authenticate(&self, username: &str, password: &str) -> Result<Arc<AclUser>, AclError> {
        self.verify_invariants();

        let user = self.users.get(username).ok_or(AclError::AuthFailed)?;

        // Redis returns WRONGPASS for both disabled users and wrong passwords
        // (security: don't reveal whether user exists or is disabled)
        if !user.enabled {
            return Err(AclError::AuthFailed);
        }

        let password_hash = Self::hash_password(password);
        if !user.verify_password(&password_hash) {
            return Err(AclError::AuthFailed);
        }

        Ok(Arc::clone(user))
    }

    /// Check if a user is permitted to execute a command on the given keys
    pub fn check_command(
        &self,
        user: Option<&AclUser>,
        command: &str,
        keys: &[&str],
    ) -> Result<(), AclError> {
        // If no user context and auth is required, reject
        if self.require_auth && user.is_none() {
            return Err(AclError::NotAuthenticated);
        }

        // If no auth required and no user, allow (backwards compatibility)
        let user = match user {
            Some(u) => u,
            None => return Ok(()),
        };

        if !user.enabled {
            return Err(AclError::UserDisabled);
        }

        // Check command permission
        let cmd_upper = command.to_uppercase();
        if !user.commands.is_command_permitted(&cmd_upper) {
            return Err(AclError::CommandNotPermitted {
                command: cmd_upper,
                user: user.name.clone(),
            });
        }

        // Check key permissions
        for key in keys {
            if !user.keys.is_key_permitted(key) {
                return Err(AclError::KeyNotPermitted {
                    key: (*key).to_string(),
                    user: user.name.clone(),
                });
            }
        }

        Ok(())
    }

    /// Verify structural invariants of the ACL state.
    /// Called after mutations in debug builds.
    #[cfg(debug_assertions)]
    pub fn verify_invariants(&self) {
        // INV-1: Default user always exists
        debug_assert!(
            self.users.contains_key("default"),
            "ACL invariant violated: default user must always exist"
        );
        // INV-2: Default user is always enabled
        debug_assert!(
            self.users["default"].enabled,
            "ACL invariant violated: default user must always be enabled"
        );
        // INV-3: Password hashes are valid SHA256 hex (64 chars)
        for user in self.users.values() {
            for hash in &user.password_hashes {
                debug_assert_eq!(
                    hash.len(),
                    64,
                    "ACL invariant violated: password hash for '{}' has length {} (expected 64)",
                    user.name,
                    hash.len()
                );
                debug_assert!(
                    hash.chars().all(|c| c.is_ascii_hexdigit()),
                    "ACL invariant violated: password hash for '{}' contains non-hex chars",
                    user.name
                );
            }
        }
        // INV-4: No empty username
        for name in self.users.keys() {
            debug_assert!(
                !name.is_empty(),
                "ACL invariant violated: empty username found"
            );
        }
    }

    /// No-op in release builds
    #[cfg(not(debug_assertions))]
    pub fn verify_invariants(&self) {}

    /// Add or update a user
    pub fn set_user(&mut self, user: AclUser) {
        self.users.insert(user.name.clone(), Arc::new(user));
        self.verify_invariants();
    }

    /// Get a user by name
    pub fn get_user(&self, username: &str) -> Option<Arc<AclUser>> {
        self.users.get(username).cloned()
    }

    /// Delete a user (cannot delete "default" user)
    pub fn del_user(&mut self, username: &str) -> Result<bool, String> {
        if username == "default" {
            return Err("ERR The 'default' user cannot be removed".to_string());
        }
        let removed = self.users.remove(username).is_some();
        self.verify_invariants();
        Ok(removed)
    }

    /// List all users
    pub fn list_users(&self) -> Vec<Arc<AclUser>> {
        self.users.values().cloned().collect()
    }

    /// Get user names
    pub fn user_names(&self) -> Vec<&str> {
        self.users.keys().map(|s| s.as_str()).collect()
    }

    /// Get the default user
    pub fn default_user(&self) -> Arc<AclUser> {
        self.users
            .get("default")
            .cloned()
            .expect("default user must always exist")
    }
}

impl Default for AclManager {
    fn default() -> Self {
        Self::new()
    }
}

// hex encoding helper (avoid adding another dependency)
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing() {
        let hash1 = AclManager::hash_password("test123");
        let hash2 = AclManager::hash_password("test123");
        let hash3 = AclManager::hash_password("different");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA256 produces 64 hex chars
    }

    #[test]
    fn test_default_user() {
        let manager = AclManager::new();
        let default = manager.default_user();
        assert_eq!(default.name, "default");
        assert!(default.enabled);
    }

    #[test]
    fn test_authentication() {
        let mut manager = AclManager::new();

        // Create a user with password
        let mut user = AclUser::new("alice".to_string());
        user.add_password("secret123");
        user.enabled = true;
        manager.set_user(user);

        // Successful auth
        let result = manager.authenticate("alice", "secret123");
        assert!(result.is_ok());

        // Wrong password
        let result = manager.authenticate("alice", "wrong");
        assert!(matches!(result, Err(AclError::AuthFailed)));

        // Non-existent user
        let result = manager.authenticate("bob", "anything");
        assert!(matches!(result, Err(AclError::AuthFailed)));
    }

    #[test]
    fn test_command_check() {
        let mut manager = AclManager::new();

        // Create a read-only user
        let mut user = AclUser::new("readonly".to_string());
        user.commands.allow_all = false;
        user.commands.add_category(CommandCategory::Read);
        user.keys = KeyPatterns::allow_all(); // Allow all keys
        user.enabled = true;
        manager.set_user(user.clone());

        // GET should be allowed (read command)
        let result = manager.check_command(Some(&user), "GET", &["mykey"]);
        assert!(result.is_ok(), "GET should be allowed: {:?}", result);

        // SET should be denied (write command)
        let result = manager.check_command(Some(&user), "SET", &["mykey"]);
        assert!(matches!(result, Err(AclError::CommandNotPermitted { .. })));
    }

    #[test]
    fn test_acl_log_store_basic() {
        let mut store = AclLogStore::new();

        store.record_denial("alice", AclLogReason::Command, "SET", "toplevel", "127.0.0.1:1234");
        let entries = store.get_log(None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].count, 1);
        assert_eq!(entries[0].username, "alice");
        assert_eq!(entries[0].object, "SET");
        assert_eq!(entries[0].reason, AclLogReason::Command);
    }

    #[test]
    fn test_acl_log_store_aggregation() {
        let mut store = AclLogStore::new();

        // Same denial aggregates
        store.record_denial("alice", AclLogReason::Command, "SET", "toplevel", "127.0.0.1:1234");
        store.record_denial("alice", AclLogReason::Command, "SET", "toplevel", "127.0.0.1:1234");
        store.record_denial("alice", AclLogReason::Command, "SET", "toplevel", "127.0.0.1:1234");

        let entries = store.get_log(None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].count, 3);

        // Different denial creates new entry
        store.record_denial("bob", AclLogReason::Key, "mykey", "toplevel", "127.0.0.1:5678");
        let entries = store.get_log(None);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_acl_log_store_count() {
        let mut store = AclLogStore::new();

        store.record_denial("alice", AclLogReason::Command, "SET", "toplevel", "addr1");
        store.record_denial("bob", AclLogReason::Command, "DEL", "toplevel", "addr2");
        store.record_denial("charlie", AclLogReason::Key, "secret", "toplevel", "addr3");

        // Get only 2 most recent
        let entries = store.get_log(Some(2));
        assert_eq!(entries.len(), 2);
        // Most recent first
        assert_eq!(entries[0].username, "charlie");
        assert_eq!(entries[1].username, "bob");
    }

    #[test]
    fn test_acl_log_store_reset() {
        let mut store = AclLogStore::new();

        store.record_denial("alice", AclLogReason::Command, "SET", "toplevel", "addr");
        assert_eq!(store.get_log(None).len(), 1);

        store.reset();
        assert_eq!(store.get_log(None).len(), 0);

        // After reset, new entries still work
        store.record_denial("bob", AclLogReason::Command, "GET", "toplevel", "addr2");
        assert_eq!(store.get_log(None).len(), 1);
    }

    #[test]
    fn test_acl_dryrun() {
        use super::commands::AclCommandHandler;

        let mut manager = AclManager::new();

        // Create a read-only user
        let mut user = AclUser::new("reader".to_string());
        user.commands.allow_all = false;
        user.commands.add_category(CommandCategory::Read);
        user.keys = KeyPatterns::allow_all();
        user.enabled = true;
        manager.set_user(user);

        // GET should be permitted
        let result = AclCommandHandler::handle_dryrun(&manager, "reader", "GET", &["mykey".to_string()]);
        assert!(result.is_ok());

        // SET should be denied
        let result = AclCommandHandler::handle_dryrun(&manager, "reader", "SET", &["mykey".to_string(), "val".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no permissions to run"));

        // Non-existent user should error
        let result = AclCommandHandler::handle_dryrun(&manager, "ghost", "GET", &["key".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
