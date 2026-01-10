//! Redis 6.0+ compatible ACL (Access Control List) system
//!
//! Provides user authentication and command authorization with:
//! - Multiple users with passwords (SHA256 hashed)
//! - Per-user command permissions (allow/deny lists, categories)
//! - Per-user key pattern restrictions
//! - Default user for backwards compatibility

mod commands;
mod patterns;
mod user;

pub use commands::AclCommandHandler;
pub use patterns::{KeyPattern, KeyPatterns};
pub use user::{AclUser, CommandCategory, CommandPermissions};

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

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
            AclError::AuthFailed => write!(f, "WRONGPASS invalid username-password pair"),
            AclError::UserDisabled => write!(f, "NOPERM user is disabled"),
            AclError::CommandNotPermitted { command, user } => {
                write!(
                    f,
                    "NOPERM User {} has no permissions to run the '{}' command",
                    user, command
                )
            }
            AclError::KeyNotPermitted { key, user } => {
                write!(
                    f,
                    "NOPERM User {} has no permissions to access the '{}' key",
                    user, key
                )
            }
            AclError::UserNotFound { username } => {
                write!(f, "ERR User {} not found", username)
            }
            AclError::UserAlreadyExists { username } => {
                write!(f, "ERR User {} already exists", username)
            }
            AclError::InvalidRule { rule, reason } => {
                write!(f, "ERR Invalid ACL rule '{}': {}", rule, reason)
            }
            AclError::NotAuthenticated => {
                write!(f, "NOAUTH Authentication required")
            }
        }
    }
}

impl std::error::Error for AclError {}

/// ACL Manager - manages users and authorization
#[derive(Debug)]
pub struct AclManager {
    /// All registered users
    users: HashMap<String, Arc<AclUser>>,
    /// Whether authentication is required for new connections
    require_auth: bool,
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
        let user = self
            .users
            .get(username)
            .ok_or(AclError::AuthFailed)?;

        if !user.enabled {
            return Err(AclError::UserDisabled);
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

    /// Add or update a user
    pub fn set_user(&mut self, user: AclUser) {
        self.users.insert(user.name.clone(), Arc::new(user));
    }

    /// Get a user by name
    pub fn get_user(&self, username: &str) -> Option<Arc<AclUser>> {
        self.users.get(username).cloned()
    }

    /// Delete a user (cannot delete "default" user)
    pub fn del_user(&mut self, username: &str) -> Result<bool, AclError> {
        if username == "default" {
            return Err(AclError::InvalidRule {
                rule: "deluser default".to_string(),
                reason: "cannot delete the default user".to_string(),
            });
        }
        Ok(self.users.remove(username).is_some())
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
}
