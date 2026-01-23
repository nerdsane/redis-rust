//! ACL User and permission types

use super::patterns::KeyPatterns;
use std::collections::HashSet;

/// Command categories (like @read, @write, @admin in Redis)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    /// Read-only commands (GET, MGET, HGET, etc.)
    Read,
    /// Write commands (SET, DEL, HSET, etc.)
    Write,
    /// Administrative commands (CONFIG, DEBUG, SHUTDOWN, etc.)
    Admin,
    /// Dangerous commands (FLUSHALL, FLUSHDB, etc.)
    Dangerous,
    /// Key-space commands (KEYS, SCAN, etc.)
    Keyspace,
    /// String commands
    String,
    /// List commands
    List,
    /// Set commands
    Set,
    /// Hash commands
    Hash,
    /// Sorted set commands
    SortedSet,
    /// Connection commands (AUTH, PING, etc.)
    Connection,
    /// Server commands (INFO, DBSIZE, etc.)
    Server,
    /// Scripting commands (EVAL, EVALSHA, etc.)
    Scripting,
    /// Transaction commands (MULTI, EXEC, etc.) - not implemented
    Transaction,
    /// All commands
    All,
}

impl CommandCategory {
    /// Get commands in this category
    pub fn commands(&self) -> &'static [&'static str] {
        match self {
            CommandCategory::Read => &[
                "GET",
                "MGET",
                "HGET",
                "HGETALL",
                "HKEYS",
                "HVALS",
                "HLEN",
                "HEXISTS",
                "LRANGE",
                "LINDEX",
                "LLEN",
                "SMEMBERS",
                "SISMEMBER",
                "SCARD",
                "ZRANGE",
                "ZREVRANGE",
                "ZSCORE",
                "ZRANK",
                "ZCARD",
                "ZCOUNT",
                "ZRANGEBYSCORE",
                "STRLEN",
                "EXISTS",
                "TYPE",
                "TTL",
                "PTTL",
                "SCAN",
                "HSCAN",
                "ZSCAN",
                "KEYS",
                "DBSIZE",
                "INFO",
            ],
            CommandCategory::Write => &[
                "SET",
                "SETEX",
                "SETNX",
                "MSET",
                "APPEND",
                "GETSET",
                "INCR",
                "DECR",
                "INCRBY",
                "DECRBY",
                "LPUSH",
                "RPUSH",
                "LPOP",
                "RPOP",
                "LSET",
                "LTRIM",
                "RPOPLPUSH",
                "LMOVE",
                "HSET",
                "HDEL",
                "HINCRBY",
                "SADD",
                "SREM",
                "ZADD",
                "ZREM",
                "DEL",
                "EXPIRE",
                "EXPIREAT",
                "PEXPIREAT",
                "PERSIST",
            ],
            CommandCategory::Admin => &[
                "CONFIG",
                "DEBUG",
                "SHUTDOWN",
                "SLAVEOF",
                "REPLICAOF",
                "BGREWRITEAOF",
                "BGSAVE",
                "SAVE",
                "LASTSAVE",
            ],
            CommandCategory::Dangerous => &["FLUSHALL", "FLUSHDB", "DEBUG", "SHUTDOWN"],
            CommandCategory::Keyspace => &[
                "KEYS",
                "SCAN",
                "HSCAN",
                "ZSCAN",
                "DEL",
                "EXISTS",
                "TYPE",
                "EXPIRE",
                "EXPIREAT",
                "PEXPIREAT",
                "TTL",
                "PTTL",
                "PERSIST",
            ],
            CommandCategory::String => &[
                "GET", "SET", "SETEX", "SETNX", "MGET", "MSET", "APPEND", "GETSET", "STRLEN",
                "GETRANGE", "INCR", "DECR", "INCRBY", "DECRBY",
            ],
            CommandCategory::List => &[
                "LPUSH",
                "RPUSH",
                "LPOP",
                "RPOP",
                "LRANGE",
                "LINDEX",
                "LLEN",
                "LSET",
                "LTRIM",
                "RPOPLPUSH",
                "LMOVE",
            ],
            CommandCategory::Set => &["SADD", "SREM", "SMEMBERS", "SISMEMBER", "SCARD"],
            CommandCategory::Hash => &[
                "HSET", "HGET", "HDEL", "HGETALL", "HKEYS", "HVALS", "HLEN", "HEXISTS", "HINCRBY",
                "HSCAN",
            ],
            CommandCategory::SortedSet => &[
                "ZADD",
                "ZREM",
                "ZSCORE",
                "ZRANK",
                "ZRANGE",
                "ZREVRANGE",
                "ZCARD",
                "ZCOUNT",
                "ZRANGEBYSCORE",
                "ZSCAN",
            ],
            CommandCategory::Connection => &["AUTH", "PING", "ECHO", "SELECT", "QUIT"],
            CommandCategory::Server => &["INFO", "DBSIZE", "TIME", "COMMAND"],
            CommandCategory::Scripting => &["EVAL", "EVALSHA", "SCRIPT"],
            CommandCategory::Transaction => &["MULTI", "EXEC", "DISCARD", "WATCH", "UNWATCH"],
            CommandCategory::All => &[], // Special: allows everything
        }
    }

    /// Parse category from string (e.g., "@read", "@write")
    pub fn from_str(s: &str) -> Option<Self> {
        let s = s.strip_prefix('@').unwrap_or(s);
        match s.to_lowercase().as_str() {
            "read" => Some(CommandCategory::Read),
            "write" => Some(CommandCategory::Write),
            "admin" => Some(CommandCategory::Admin),
            "dangerous" => Some(CommandCategory::Dangerous),
            "keyspace" => Some(CommandCategory::Keyspace),
            "string" => Some(CommandCategory::String),
            "list" => Some(CommandCategory::List),
            "set" => Some(CommandCategory::Set),
            "hash" => Some(CommandCategory::Hash),
            "sortedset" | "zset" => Some(CommandCategory::SortedSet),
            "connection" => Some(CommandCategory::Connection),
            "server" => Some(CommandCategory::Server),
            "scripting" => Some(CommandCategory::Scripting),
            "transaction" => Some(CommandCategory::Transaction),
            "all" | "allcommands" => Some(CommandCategory::All),
            _ => None,
        }
    }
}

/// Command permissions for a user
#[derive(Debug, Clone)]
pub struct CommandPermissions {
    /// Allow all commands by default
    pub allow_all: bool,
    /// Explicitly denied commands (takes precedence)
    pub denied: HashSet<String>,
    /// Explicitly allowed commands
    pub allowed: HashSet<String>,
    /// Allowed categories
    pub categories: HashSet<CommandCategory>,
    /// Denied categories
    pub denied_categories: HashSet<CommandCategory>,
}

impl CommandPermissions {
    /// Create permissions that allow all commands
    pub fn allow_all() -> Self {
        Self {
            allow_all: true,
            denied: HashSet::new(),
            allowed: HashSet::new(),
            categories: HashSet::new(),
            denied_categories: HashSet::new(),
        }
    }

    /// Create permissions that deny all commands by default
    pub fn deny_all() -> Self {
        Self {
            allow_all: false,
            denied: HashSet::new(),
            allowed: HashSet::new(),
            categories: HashSet::new(),
            denied_categories: HashSet::new(),
        }
    }

    /// Add a command category
    pub fn add_category(&mut self, category: CommandCategory) {
        self.categories.insert(category);
    }

    /// Remove a command category
    pub fn remove_category(&mut self, category: CommandCategory) {
        self.categories.remove(&category);
        self.denied_categories.insert(category);
    }

    /// Allow a specific command
    pub fn allow_command(&mut self, command: &str) {
        let cmd = command.to_uppercase();
        self.denied.remove(&cmd);
        self.allowed.insert(cmd);
    }

    /// Deny a specific command
    pub fn deny_command(&mut self, command: &str) {
        let cmd = command.to_uppercase();
        self.allowed.remove(&cmd);
        self.denied.insert(cmd);
    }

    /// Check if a command is permitted
    pub fn is_command_permitted(&self, command: &str) -> bool {
        let cmd = command.to_uppercase();

        // Explicit deny takes precedence
        if self.denied.contains(&cmd) {
            return false;
        }

        // Explicit allow
        if self.allowed.contains(&cmd) {
            return true;
        }

        // Check denied categories
        for cat in &self.denied_categories {
            if cat.commands().contains(&cmd.as_str()) {
                return false;
            }
        }

        // Check allowed categories
        for cat in &self.categories {
            if *cat == CommandCategory::All {
                return true;
            }
            if cat.commands().contains(&cmd.as_str()) {
                return true;
            }
        }

        // Fall back to allow_all setting
        self.allow_all
    }
}

impl Default for CommandPermissions {
    fn default() -> Self {
        Self::allow_all()
    }
}

/// A user in the ACL system
#[derive(Debug, Clone)]
pub struct AclUser {
    /// Username
    pub name: String,
    /// Password hashes (SHA256, hex encoded)
    pub password_hashes: Vec<String>,
    /// Whether the user is enabled
    pub enabled: bool,
    /// Command permissions
    pub commands: CommandPermissions,
    /// Key access patterns
    pub keys: KeyPatterns,
    /// Whether this user can authenticate without password (nopass)
    pub nopass: bool,
}

impl AclUser {
    /// Create a new disabled user with no permissions
    pub fn new(name: String) -> Self {
        Self {
            name,
            password_hashes: Vec::new(),
            enabled: false,
            commands: CommandPermissions::deny_all(),
            keys: KeyPatterns::deny_all(),
            nopass: false,
        }
    }

    /// Create the default user (enabled, all commands, all keys)
    pub fn default_user() -> Self {
        Self {
            name: "default".to_string(),
            password_hashes: Vec::new(),
            enabled: true,
            commands: CommandPermissions::allow_all(),
            keys: KeyPatterns::allow_all(),
            nopass: true, // Default user has nopass by default
        }
    }

    /// Add a password hash to this user
    pub fn add_password(&mut self, password: &str) {
        let hash = super::AclManager::hash_password(password);
        if !self.password_hashes.contains(&hash) {
            self.password_hashes.push(hash);
        }
    }

    /// Add a pre-hashed password
    pub fn add_password_hash(&mut self, hash: String) {
        if !self.password_hashes.contains(&hash) {
            self.password_hashes.push(hash);
        }
    }

    /// Remove a password
    pub fn remove_password(&mut self, password: &str) {
        let hash = super::AclManager::hash_password(password);
        self.password_hashes.retain(|h| h != &hash);
    }

    /// Clear all passwords
    pub fn clear_passwords(&mut self) {
        self.password_hashes.clear();
    }

    /// Verify a password hash
    pub fn verify_password(&self, password_hash: &str) -> bool {
        // If nopass is set, any password works (or no password)
        if self.nopass {
            return true;
        }
        self.password_hashes.contains(&password_hash.to_string())
    }

    /// Reset user to default state (disabled, no permissions)
    pub fn reset(&mut self) {
        self.password_hashes.clear();
        self.enabled = false;
        self.commands = CommandPermissions::deny_all();
        self.keys = KeyPatterns::deny_all();
        self.nopass = false;
    }

    /// Format user as ACL rule string (for ACL LIST)
    pub fn to_acl_string(&self) -> String {
        let mut parts = Vec::new();

        // User name
        parts.push(format!("user {}", self.name));

        // Enabled/disabled
        if self.enabled {
            parts.push("on".to_string());
        } else {
            parts.push("off".to_string());
        }

        // Passwords
        if self.nopass {
            parts.push("nopass".to_string());
        }
        for hash in &self.password_hashes {
            parts.push(format!("#{}", hash));
        }

        // Commands
        if self.commands.allow_all {
            parts.push("+@all".to_string());
        }
        for cat in &self.commands.categories {
            parts.push(format!("+@{:?}", cat).to_lowercase());
        }
        for cat in &self.commands.denied_categories {
            parts.push(format!("-@{:?}", cat).to_lowercase());
        }
        for cmd in &self.commands.allowed {
            parts.push(format!("+{}", cmd.to_lowercase()));
        }
        for cmd in &self.commands.denied {
            parts.push(format!("-{}", cmd.to_lowercase()));
        }

        // Keys
        if self.keys.allow_all {
            parts.push("~*".to_string());
        } else {
            for pattern in &self.keys.patterns {
                parts.push(format!("~{}", pattern.pattern));
            }
        }

        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_category_parse() {
        assert_eq!(
            CommandCategory::from_str("@read"),
            Some(CommandCategory::Read)
        );
        assert_eq!(
            CommandCategory::from_str("write"),
            Some(CommandCategory::Write)
        );
        assert_eq!(
            CommandCategory::from_str("@ALL"),
            Some(CommandCategory::All)
        );
        assert_eq!(CommandCategory::from_str("invalid"), None);
    }

    #[test]
    fn test_command_permissions() {
        let mut perms = CommandPermissions::deny_all();
        perms.add_category(CommandCategory::Read);

        assert!(perms.is_command_permitted("GET"));
        assert!(perms.is_command_permitted("MGET"));
        assert!(!perms.is_command_permitted("SET"));
        assert!(!perms.is_command_permitted("DEL"));

        // Explicit allow overrides
        perms.allow_command("SET");
        assert!(perms.is_command_permitted("SET"));

        // Explicit deny takes precedence
        perms.deny_command("GET");
        assert!(!perms.is_command_permitted("GET"));
    }

    #[test]
    fn test_user_password() {
        let mut user = AclUser::new("test".to_string());
        user.add_password("secret");

        let hash = super::super::AclManager::hash_password("secret");
        assert!(user.verify_password(&hash));

        let wrong_hash = super::super::AclManager::hash_password("wrong");
        assert!(!user.verify_password(&wrong_hash));
    }

    #[test]
    fn test_nopass_user() {
        let mut user = AclUser::new("test".to_string());
        user.nopass = true;

        // Any password should work
        assert!(user.verify_password("anything"));
        assert!(user.verify_password(""));
    }
}
