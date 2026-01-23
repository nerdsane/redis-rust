//! ACL file loading and parsing
//!
//! Redis ACL file format (one user per line):
//! ```text
//! user <username> [on|off] [nopass|>password|#hash] [+@category|-@category] [+cmd|-cmd] [~pattern]
//! ```
//!
//! Example:
//! ```text
//! user default on nopass ~* +@all
//! user alice on >secretpassword ~cache:* +@read +@connection -@dangerous
//! user bob off #9f735e0df9a1ddc702bf0a1a7b83033f9f7153a00c29de82cedadc9957289b05 ~* +@all
//! ```

use super::commands::apply_rule;
use super::{AclError, AclUser};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Errors that can occur when loading ACL files
#[derive(Debug)]
pub enum AclFileError {
    /// IO error reading the file
    IoError {
        path: String,
        source: std::io::Error,
    },
    /// Parse error on a specific line
    ParseError {
        path: String,
        line_number: usize,
        line: String,
        reason: String,
    },
    /// ACL rule error
    RuleError {
        path: String,
        line_number: usize,
        error: AclError,
    },
}

impl std::fmt::Display for AclFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AclFileError::IoError { path, source } => {
                write!(f, "Failed to read ACL file '{}': {}", path, source)
            }
            AclFileError::ParseError {
                path,
                line_number,
                line,
                reason,
            } => {
                write!(
                    f,
                    "Parse error in ACL file '{}' line {}: {} (line: '{}')",
                    path, line_number, reason, line
                )
            }
            AclFileError::RuleError {
                path,
                line_number,
                error,
            } => {
                write!(
                    f,
                    "ACL rule error in '{}' line {}: {}",
                    path, line_number, error
                )
            }
        }
    }
}

impl std::error::Error for AclFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AclFileError::IoError { source, .. } => Some(source),
            AclFileError::RuleError { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Load users from an ACL file
///
/// Returns a vector of (username, AclUser) pairs.
/// The file format is one user definition per line.
pub fn load_acl_file(path: impl AsRef<Path>) -> Result<Vec<AclUser>, AclFileError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();

    let file = File::open(path).map_err(|e| AclFileError::IoError {
        path: path_str.clone(),
        source: e,
    })?;

    let reader = BufReader::new(file);
    let mut users = Vec::new();

    for (line_number, line_result) in reader.lines().enumerate() {
        let line_number = line_number + 1; // 1-indexed for error messages

        let line = line_result.map_err(|e| AclFileError::IoError {
            path: path_str.clone(),
            source: e,
        })?;

        // Skip empty lines and comments
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Parse the user line
        let user = parse_user_line(trimmed, &path_str, line_number)?;
        users.push(user);
    }

    Ok(users)
}

/// Parse a single user line from the ACL file
fn parse_user_line(line: &str, path: &str, line_number: usize) -> Result<AclUser, AclFileError> {
    let parts: Vec<&str> = line.split_whitespace().collect();

    // Must start with "user"
    if parts.is_empty() || parts[0].to_lowercase() != "user" {
        return Err(AclFileError::ParseError {
            path: path.to_string(),
            line_number,
            line: line.to_string(),
            reason: "Line must start with 'user'".to_string(),
        });
    }

    // Must have at least a username
    if parts.len() < 2 {
        return Err(AclFileError::ParseError {
            path: path.to_string(),
            line_number,
            line: line.to_string(),
            reason: "Missing username after 'user'".to_string(),
        });
    }

    let username = parts[1];
    let mut user = AclUser::new(username.to_string());

    // Apply each subsequent rule
    for rule in &parts[2..] {
        apply_rule(&mut user, rule).map_err(|e| AclFileError::RuleError {
            path: path.to_string(),
            line_number,
            error: e,
        })?;
    }

    Ok(user)
}

/// Save users to an ACL file
pub fn save_acl_file(path: impl AsRef<Path>, users: &[AclUser]) -> Result<(), AclFileError> {
    use std::io::Write;

    let path = path.as_ref();
    let path_str = path.display().to_string();

    let mut file = File::create(path).map_err(|e| AclFileError::IoError {
        path: path_str.clone(),
        source: e,
    })?;

    // Write header comment
    writeln!(file, "# Redis ACL file").map_err(|e| AclFileError::IoError {
        path: path_str.clone(),
        source: e,
    })?;
    writeln!(file, "# Generated by redis-rust").map_err(|e| AclFileError::IoError {
        path: path_str.clone(),
        source: e,
    })?;
    writeln!(file).map_err(|e| AclFileError::IoError {
        path: path_str.clone(),
        source: e,
    })?;

    // Write each user
    for user in users {
        writeln!(file, "{}", user.to_acl_string()).map_err(|e| AclFileError::IoError {
            path: path_str.clone(),
            source: e,
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_simple_user() {
        let user = parse_user_line("user alice on >secret ~* +@all", "test", 1).unwrap();
        assert_eq!(user.name, "alice");
        assert!(user.enabled);
        assert!(user.keys.allow_all);
        assert!(
            user.commands.allow_all
                || user
                    .commands
                    .categories
                    .contains(&super::super::CommandCategory::All)
        );
    }

    #[test]
    fn test_parse_disabled_user() {
        let user = parse_user_line("user bob off", "test", 1).unwrap();
        assert_eq!(user.name, "bob");
        assert!(!user.enabled);
    }

    #[test]
    fn test_parse_nopass_user() {
        let user = parse_user_line("user guest on nopass ~* +@read", "test", 1).unwrap();
        assert_eq!(user.name, "guest");
        assert!(user.enabled);
        assert!(user.nopass);
    }

    #[test]
    fn test_parse_key_patterns() {
        let user = parse_user_line(
            "user readonly on nopass ~cache:* ~session:* +@read",
            "test",
            1,
        )
        .unwrap();
        assert_eq!(user.name, "readonly");
        assert!(!user.keys.allow_all);
        assert!(user.keys.is_key_permitted("cache:foo"));
        assert!(user.keys.is_key_permitted("session:123"));
        assert!(!user.keys.is_key_permitted("admin:secret"));
    }

    #[test]
    fn test_load_acl_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# Comment line").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "user default on nopass ~* +@all").unwrap();
        writeln!(file, "user alice on >secret ~cache:* +@read").unwrap();
        writeln!(file, "user bob off").unwrap();
        file.flush().unwrap();

        let users = load_acl_file(file.path()).unwrap();
        assert_eq!(users.len(), 3);
        assert_eq!(users[0].name, "default");
        assert_eq!(users[1].name, "alice");
        assert_eq!(users[2].name, "bob");
    }

    #[test]
    fn test_invalid_line() {
        let result = parse_user_line("invalid line", "test", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_username() {
        let result = parse_user_line("user", "test", 1);
        assert!(result.is_err());
    }
}
