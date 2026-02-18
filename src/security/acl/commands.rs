//! ACL command handlers

use super::{AclError, AclManager, AclUser, CommandCategory, KeyPattern};
use std::sync::Arc;

/// Handler for ACL-related commands
pub struct AclCommandHandler;

impl AclCommandHandler {
    /// Handle AUTH command
    /// Returns the authenticated user on success
    pub fn handle_auth(manager: &AclManager, args: &[&str]) -> Result<Arc<AclUser>, AclError> {
        match args.len() {
            // AUTH password (authenticate as default user)
            1 => manager.authenticate("default", args[0]),
            // AUTH username password
            2 => manager.authenticate(args[0], args[1]),
            _ => Err(AclError::InvalidRule {
                rule: "AUTH".to_string(),
                reason: "wrong number of arguments".to_string(),
            }),
        }
    }

    /// Handle ACL WHOAMI command
    pub fn handle_whoami(user: Option<&AclUser>) -> String {
        match user {
            Some(u) => u.name.clone(),
            None => "default".to_string(),
        }
    }

    /// Handle ACL LIST command
    pub fn handle_list(manager: &AclManager) -> Vec<String> {
        let mut users = manager.list_users();
        users.sort_by(|a, b| a.name.cmp(&b.name));
        users.iter().map(|u| u.to_acl_string()).collect()
    }

    /// Handle ACL USERS command
    pub fn handle_users(manager: &AclManager) -> Vec<String> {
        let mut names: Vec<String> = manager.user_names().iter().map(|s| s.to_string()).collect();
        names.sort();
        names
    }

    /// Handle ACL GETUSER command
    /// Returns structured data for building the RESP response
    pub fn handle_getuser(manager: &AclManager, username: &str) -> Option<AclGetUserInfo> {
        manager.get_user(username).map(|user| {
            let flags = {
                let mut f = Vec::new();
                if user.enabled {
                    f.push("on".to_string());
                } else {
                    f.push("off".to_string());
                }
                if user.nopass {
                    f.push("nopass".to_string());
                }
                f
            };

            let passwords: Vec<String> = user
                .password_hashes
                .iter()
                .map(|h| format!("#{}", h))
                .collect();

            let commands = format_commands(&user);
            let keys = format_keys(&user);

            AclGetUserInfo {
                flags,
                passwords,
                commands,
                keys,
                channels: "&*".to_string(),
            }
        })
    }

    /// Handle ACL SETUSER command
    /// Parses rules and creates/modifies a user
    pub fn handle_setuser(
        manager: &mut AclManager,
        username: &str,
        rules: &[&str],
    ) -> Result<(), AclError> {
        // Validate username
        if username.contains(' ') || username.contains('\0') {
            return Err(AclError::InvalidRule {
                rule: "SETUSER".to_string(),
                reason: "Usernames can't contain spaces or null characters".to_string(),
            });
        }

        // Get existing user or create new one
        let mut user = manager
            .get_user(username)
            .map(|u| (*u).clone())
            .unwrap_or_else(|| AclUser::new(username.to_string()));

        // Apply each rule
        for rule in rules {
            apply_rule(&mut user, rule)?;
        }

        manager.set_user(user);
        Ok(())
    }

    /// Handle ACL DELUSER command
    pub fn handle_deluser(manager: &mut AclManager, usernames: &[&str]) -> Result<usize, String> {
        let mut deleted = 0;
        for username in usernames {
            if manager.del_user(username)? {
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    /// Handle ACL CAT command
    pub fn handle_cat(category: Option<&str>) -> Result<Vec<String>, AclError> {
        match category {
            None => {
                // List all categories
                Ok(vec![
                    "keyspace",
                    "read",
                    "write",
                    "set",
                    "sortedset",
                    "list",
                    "hash",
                    "string",
                    "bitmap",
                    "hyperloglog",
                    "geo",
                    "stream",
                    "pubsub",
                    "admin",
                    "fast",
                    "slow",
                    "dangerous",
                    "connection",
                    "transaction",
                    "scripting",
                    "server",
                ]
                .into_iter()
                .map(|s| s.to_string())
                .collect())
            }
            Some(cat) => {
                // List commands in category
                let category =
                    CommandCategory::from_str(cat).ok_or_else(|| AclError::InvalidRule {
                        rule: cat.to_string(),
                        reason: "unknown category".to_string(),
                    })?;
                Ok(category
                    .commands()
                    .iter()
                    .map(|s| s.to_lowercase())
                    .collect())
            }
        }
    }

    /// Handle ACL DRYRUN command
    /// Simulates running a command as the given user without executing it.
    /// Returns Ok(()) if permitted, or an error string matching Redis format.
    pub fn handle_dryrun(
        manager: &AclManager,
        username: &str,
        command: &str,
        args: &[String],
    ) -> Result<(), String> {
        debug_assert!(!username.is_empty(), "Precondition: username must not be empty");
        debug_assert!(!command.is_empty(), "Precondition: command must not be empty");

        // Look up user
        let user = manager.get_user(username).ok_or_else(|| {
            format!("ERR User '{}' not found", username)
        })?;

        // Check if user is enabled
        if !user.enabled {
            return Err(format!(
                "This user has no permissions to run the '{}' command",
                command.to_lowercase()
            ));
        }

        // Check command permission
        let cmd_upper = command.to_uppercase();
        if !user.commands.is_command_permitted(&cmd_upper) {
            return Err(format!(
                "This user has no permissions to run the '{}' command",
                command.to_lowercase()
            ));
        }

        // Check key permissions for all keys the command touches.
        // Key extraction is command-aware: MSET has keys at even positions,
        // most other commands have all args as keys (DEL, MGET) or first arg only.
        if !args.is_empty() && !user.keys.allow_all {
            let keys = extract_dryrun_keys(&cmd_upper, args);
            for key in &keys {
                if !user.keys.is_key_permitted(key) {
                    return Err(format!(
                        "This user has no permissions to access the '{}' key",
                        key
                    ));
                }
            }
        }

        Ok(())
    }

    /// Handle ACL LOG command — return log entries as structured data
    pub fn handle_log(
        manager: &AclManager,
        count: Option<usize>,
    ) -> Vec<super::AclLogEntry> {
        manager.acl_log.get_log(count).into_iter().cloned().collect()
    }

    /// Handle ACL LOG RESET command
    pub fn handle_log_reset(manager: &mut AclManager) {
        manager.acl_log.reset();
    }

    /// Handle ACL GENPASS command
    pub fn handle_genpass(bits: Option<u32>) -> Result<String, String> {
        use std::time::{SystemTime, UNIX_EPOCH};

        if let Some(b) = bits {
            if b == 0 || b > 4096 {
                return Err(
                    "ERR ACL GENPASS argument must be the number of bits for the output password, a positive number up to 4096".to_string()
                );
            }
        }

        let bits = bits.unwrap_or(256);
        let bytes = (bits as usize).div_ceil(8);

        // Simple pseudo-random generation (not cryptographically secure)
        // In production, use proper random source
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

        Ok(result)
    }
}

/// Structured GETUSER response data
pub struct AclGetUserInfo {
    pub flags: Vec<String>,
    pub passwords: Vec<String>,
    pub commands: String,
    pub keys: String,
    pub channels: String,
}

/// Apply a single ACL rule to a user
pub fn apply_rule(user: &mut AclUser, rule: &str) -> Result<(), AclError> {
    let rule = rule.trim();

    match rule {
        // Enable/disable
        "on" => user.enabled = true,
        "off" => user.enabled = false,

        // Password management
        "nopass" => user.nopass = true,
        "resetpass" => {
            user.password_hashes.clear();
            user.nopass = false;
        }

        // All permissions
        "allcommands" | "+@all" => {
            user.commands.allow_all = true;
            user.commands.categories.insert(CommandCategory::All);
        }
        "nocommands" | "-@all" => {
            user.commands.allow_all = false;
            user.commands.categories.clear();
            user.commands.allowed.clear();
        }

        // All keys
        "allkeys" | "~*" => {
            user.keys.allow_all = true;
        }
        "resetkeys" => {
            user.keys.reset();
        }

        // Channel permissions (stored but not enforced — PubSub not implemented)
        "allchannels" | "&*" => {
            // No-op: we don't enforce channel ACLs
        }
        "resetchannels" => {
            // No-op: we don't enforce channel ACLs
        }

        // Reset everything
        "reset" => {
            user.reset();
        }

        // Sanitize payload flags (no-op, accepted for compatibility)
        "sanitize-payload" | "skip-sanitize-payload" => {}


        // Pattern-based rules
        _ => {
            if let Some(rest) = rule.strip_prefix('>') {
                // Add password
                user.add_password(rest);
            } else if let Some(rest) = rule.strip_prefix('<') {
                // Remove password
                user.remove_password(rest);
            } else if let Some(rest) = rule.strip_prefix('#') {
                // Add password hash directly — validate it's a valid SHA256 hex string
                if rest.len() != 64 || !rest.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(AclError::InvalidRule {
                        rule: format!("#{}", rest),
                        reason: "Syntax error in ACL SETUSER modifier".to_string(),
                    });
                }
                user.add_password_hash(rest.to_string());
            } else if let Some(rest) = rule.strip_prefix('!') {
                // Remove password hash directly
                user.password_hashes.retain(|h| h != rest);
            } else if let Some(rest) = rule.strip_prefix('+') {
                // Allow command or category
                if let Some(cat) = rest.strip_prefix('@') {
                    if let Some(category) = CommandCategory::from_str(cat) {
                        user.commands.add_category(category);
                    } else {
                        return Err(AclError::InvalidRule {
                            rule: rule.to_string(),
                            reason: format!("unknown category: {}", cat),
                        });
                    }
                } else if rest.contains('|') {
                    // Subcommand syntax: +command|subcommand
                    // Store the full form for display, and also allow the base command
                    user.commands.allowed.insert(rest.to_uppercase());
                } else {
                    user.commands.allow_command(rest);
                }
            } else if let Some(rest) = rule.strip_prefix('-') {
                // Deny command or category
                if let Some(cat) = rest.strip_prefix('@') {
                    if let Some(category) = CommandCategory::from_str(cat) {
                        user.commands.remove_category(category);
                    } else {
                        return Err(AclError::InvalidRule {
                            rule: rule.to_string(),
                            reason: format!("unknown category: {}", cat),
                        });
                    }
                } else if rest.contains('|') {
                    // Subcommand syntax: -command|subcommand
                    user.commands.denied.insert(rest.to_uppercase());
                } else {
                    user.commands.deny_command(rest);
                }
            } else if let Some(rest) = rule.strip_prefix('~') {
                // Key pattern
                user.keys.add_pattern(KeyPattern::new(rest.to_string()));
            } else if rule.starts_with('&') {
                // Channel pattern — no-op (PubSub not implemented)
            } else if let Some(rest) = rule.strip_prefix('%') {
                // Read/write key pattern (advanced feature)
                if let Some(pattern) = rest.strip_prefix("R~") {
                    user.keys
                        .add_pattern(KeyPattern::read_only(pattern.to_string()));
                } else if let Some(pattern) = rest.strip_prefix("W~") {
                    user.keys
                        .add_pattern(KeyPattern::write_only(pattern.to_string()));
                } else if let Some(pattern) = rest.strip_prefix("RW~") {
                    user.keys.add_pattern(KeyPattern::new(pattern.to_string()));
                } else {
                    return Err(AclError::InvalidRule {
                        rule: rule.to_string(),
                        reason: "invalid key permission format".to_string(),
                    });
                }
            } else {
                return Err(AclError::InvalidRule {
                    rule: rule.to_string(),
                    reason: "unrecognized rule".to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Extract key arguments from a DRYRUN command invocation.
/// Command-aware: handles multi-key commands like MSET, DEL, MGET, RENAME.
fn extract_dryrun_keys<'a>(command: &str, args: &'a [String]) -> Vec<&'a str> {
    match command {
        // MSET key val key val ... — keys at even indices (0, 2, 4, ...)
        "MSET" | "MSETNX" => args
            .iter()
            .step_by(2)
            .map(|s| s.as_str())
            .collect(),
        // All args are keys
        "DEL" | "UNLINK" | "EXISTS" | "MGET" | "WATCH" => {
            args.iter().map(|s| s.as_str()).collect()
        }
        // Two-key commands: source + dest
        "RENAME" | "RENAMENX" | "RPOPLPUSH" => {
            args.iter().take(2).map(|s| s.as_str()).collect()
        }
        // LMOVE source dest wherefrom whereto — first 2 args are keys
        "LMOVE" => args.iter().take(2).map(|s| s.as_str()).collect(),
        // Default: first arg is the key (GET, SET, HSET, LPUSH, ZADD, etc.)
        _ => {
            if args.is_empty() {
                vec![]
            } else {
                vec![args[0].as_str()]
            }
        }
    }
}

fn format_flags(user: &AclUser) -> String {
    let mut flags = Vec::new();
    if user.enabled {
        flags.push("on");
    } else {
        flags.push("off");
    }
    if user.nopass {
        flags.push("nopass");
    }
    if user.commands.allow_all {
        flags.push("allcommands");
    }
    if user.keys.allow_all {
        flags.push("allkeys");
    }
    flags.join(" ")
}

fn format_passwords(user: &AclUser) -> String {
    if user.password_hashes.is_empty() {
        "(empty)".to_string()
    } else {
        user.password_hashes
            .iter()
            .map(|h| format!("#{}", h))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn format_commands(user: &AclUser) -> String {
    let mut parts = Vec::new();
    if user.commands.allow_all {
        parts.push("+@all".to_string());
    }
    for cat in &user.commands.categories {
        parts.push(format!("+@{}", format_category_name_cmd(cat)));
    }
    for cat in &user.commands.denied_categories {
        parts.push(format!("-@{}", format_category_name_cmd(cat)));
    }
    for cmd in &user.commands.allowed {
        parts.push(format!("+{}", cmd.to_lowercase()));
    }
    for cmd in &user.commands.denied {
        parts.push(format!("-{}", cmd.to_lowercase()));
    }
    if parts.is_empty() {
        "-@all".to_string()
    } else {
        parts.join(" ")
    }
}

fn format_category_name_cmd(cat: &CommandCategory) -> &'static str {
    match cat {
        CommandCategory::Read => "read",
        CommandCategory::Write => "write",
        CommandCategory::Admin => "admin",
        CommandCategory::Dangerous => "dangerous",
        CommandCategory::Keyspace => "keyspace",
        CommandCategory::String => "string",
        CommandCategory::List => "list",
        CommandCategory::Set => "set",
        CommandCategory::Hash => "hash",
        CommandCategory::SortedSet => "sortedset",
        CommandCategory::Connection => "connection",
        CommandCategory::Server => "server",
        CommandCategory::Scripting => "scripting",
        CommandCategory::Transaction => "transaction",
        CommandCategory::PubSub => "pubsub",
        CommandCategory::Slow => "slow",
        CommandCategory::Fast => "fast",
        CommandCategory::Bitmap => "bitmap",
        CommandCategory::Hyperloglog => "hyperloglog",
        CommandCategory::Geo => "geo",
        CommandCategory::Stream => "stream",
        CommandCategory::All => "all",
    }
}

fn format_keys(user: &AclUser) -> String {
    if user.keys.allow_all {
        "~*".to_string()
    } else if user.keys.patterns.is_empty() {
        "(empty)".to_string()
    } else {
        user.keys
            .patterns
            .iter()
            .map(|p| format!("~{}", p.pattern))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_rules() {
        let mut user = AclUser::new("test".to_string());

        // Enable and set password
        apply_rule(&mut user, "on").unwrap();
        apply_rule(&mut user, ">secret").unwrap();

        assert!(user.enabled);
        assert_eq!(user.password_hashes.len(), 1);

        // Add read permissions
        apply_rule(&mut user, "+@read").unwrap();
        assert!(user.commands.categories.contains(&CommandCategory::Read));

        // Add key pattern
        apply_rule(&mut user, "~user:*").unwrap();
        assert_eq!(user.keys.patterns.len(), 1);
        assert!(user.keys.is_key_permitted("user:123"));
        assert!(!user.keys.is_key_permitted("admin:secret"));
    }

    #[test]
    fn test_auth_handler() {
        let mut manager = AclManager::new();

        // Create a user
        let mut user = AclUser::new("alice".to_string());
        user.add_password("secret");
        user.enabled = true;
        manager.set_user(user);

        // Test AUTH with username password
        let result = AclCommandHandler::handle_auth(&manager, &["alice", "secret"]);
        assert!(result.is_ok());

        // Test AUTH with wrong password
        let result = AclCommandHandler::handle_auth(&manager, &["alice", "wrong"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_setuser_handler() {
        let mut manager = AclManager::new();

        // Create user with rules
        AclCommandHandler::handle_setuser(
            &mut manager,
            "newuser",
            &["on", ">password", "+@read", "~cache:*"],
        )
        .unwrap();

        let user = manager.get_user("newuser").unwrap();
        assert!(user.enabled);
        assert!(user.commands.categories.contains(&CommandCategory::Read));
        assert!(user.keys.is_key_permitted("cache:foo"));
    }
}
