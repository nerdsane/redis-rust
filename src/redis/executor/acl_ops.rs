//! ACL command implementations for CommandExecutor.
//!
//! Handles: AUTH, ACL WHOAMI, ACL LIST, ACL USERS, ACL GETUSER, ACL SETUSER,
//! ACL DELUSER, ACL CAT, ACL GENPASS
//!
//! # TigerStyle Invariants
//!
//! - ACL WHOAMI always returns a valid username (at least "default")
//! - ACL LIST always returns at least the default user entry
//! - ACL USERS always returns at least "default"
//! - ACL GENPASS output length matches requested bits/4 (hex encoding)

use super::CommandExecutor;
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_auth(&self) -> RespValue {
        // AUTH is handled at the connection level
        // If we get here, it means the server doesn't have ACL enabled
        // TigerStyle: Postcondition - always succeeds without ACL feature
        RespValue::simple("OK")
    }

    pub(super) fn execute_acl_whoami(&self) -> RespValue {
        // Without ACL feature, default user is always authenticated
        let username = b"default".to_vec();

        // TigerStyle: Postcondition - username must not be empty
        debug_assert!(
            !username.is_empty(),
            "Postcondition violated: ACL WHOAMI must return non-empty username"
        );

        RespValue::BulkString(Some(username))
    }

    pub(super) fn execute_acl_list(&self) -> RespValue {
        // Return default user rule
        let rule = "user default on nopass ~* +@all".to_string();
        let rules = vec![RespValue::BulkString(Some(rule.into_bytes()))];

        // TigerStyle: Postcondition - must have at least one rule
        debug_assert!(
            !rules.is_empty(),
            "Postcondition violated: ACL LIST must return at least default user"
        );

        RespValue::Array(Some(rules))
    }

    pub(super) fn execute_acl_users(&self) -> RespValue {
        let users = vec![RespValue::BulkString(Some(b"default".to_vec()))];

        // TigerStyle: Postcondition - must have at least default user
        debug_assert!(
            !users.is_empty(),
            "Postcondition violated: ACL USERS must return at least 'default'"
        );

        RespValue::Array(Some(users))
    }

    pub(super) fn execute_acl_getuser(&self, username: &str) -> RespValue {
        if username == "default" {
            // Return info about default user
            RespValue::Array(Some(vec![
                RespValue::BulkString(Some(b"flags".to_vec())),
                RespValue::Array(Some(vec![
                    RespValue::BulkString(Some(b"on".to_vec())),
                    RespValue::BulkString(Some(b"nopass".to_vec())),
                ])),
                RespValue::BulkString(Some(b"passwords".to_vec())),
                RespValue::Array(Some(vec![])),
                RespValue::BulkString(Some(b"commands".to_vec())),
                RespValue::BulkString(Some(b"+@all".to_vec())),
                RespValue::BulkString(Some(b"keys".to_vec())),
                RespValue::BulkString(Some(b"~*".to_vec())),
            ]))
        } else {
            RespValue::BulkString(None) // User not found
        }
    }

    pub(super) fn execute_acl_setuser(&self) -> RespValue {
        // ACL management requires the ACL feature
        RespValue::err("ERR ACL feature not enabled")
    }

    pub(super) fn execute_acl_deluser(&self) -> RespValue {
        RespValue::err("ERR ACL feature not enabled")
    }

    pub(super) fn execute_acl_cat(&self, category: Option<&str>) -> RespValue {
        #[cfg(feature = "acl")]
        {
            use crate::security::acl::CommandCategory;
            match category {
                None => {
                    // List all categories
                    let categories = vec![
                        "read",
                        "write",
                        "admin",
                        "dangerous",
                        "keyspace",
                        "string",
                        "list",
                        "set",
                        "hash",
                        "sortedset",
                        "connection",
                        "server",
                        "scripting",
                        "transaction",
                    ];
                    RespValue::Array(Some(
                        categories
                            .into_iter()
                            .map(|c| RespValue::BulkString(Some(c.as_bytes().to_vec())))
                            .collect(),
                    ))
                }
                Some(cat) => {
                    // List commands in category
                    if let Some(cat_enum) = CommandCategory::from_str(cat) {
                        let commands: Vec<RespValue> = cat_enum
                            .commands()
                            .iter()
                            .map(|c| RespValue::BulkString(Some(c.to_lowercase().into_bytes())))
                            .collect();
                        RespValue::Array(Some(commands))
                    } else {
                        RespValue::err(format!("ERR Unknown ACL category '{}'", cat))
                    }
                }
            }
        }
        #[cfg(not(feature = "acl"))]
        {
            let _ = category; // Suppress unused warning
                              // Without ACL feature, return basic category list
            let categories = vec![
                "read",
                "write",
                "admin",
                "dangerous",
                "keyspace",
                "string",
                "list",
                "set",
                "hash",
                "sortedset",
                "connection",
                "server",
                "scripting",
                "transaction",
            ];
            RespValue::Array(Some(
                categories
                    .into_iter()
                    .map(|c| RespValue::BulkString(Some(c.as_bytes().to_vec())))
                    .collect(),
            ))
        }
    }

    pub(super) fn execute_acl_genpass(&self, bits: Option<u32>) -> RespValue {
        use std::time::{SystemTime, UNIX_EPOCH};

        let bits = bits.unwrap_or(256).min(1024);

        // TigerStyle: Preconditions
        debug_assert!(
            bits > 0 && bits <= 1024,
            "Precondition violated: ACL GENPASS bits must be in (0, 1024]"
        );

        let bytes = (bits as usize).div_ceil(8);
        let expected_hex_len = bytes * 2;

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time must be after UNIX epoch")
            .as_nanos();

        let mut result = String::with_capacity(expected_hex_len);
        let mut state = seed;
        for _ in 0..bytes {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let byte = ((state >> 33) & 0xFF) as u8;
            result.push_str(&format!("{:02x}", byte));
        }

        // TigerStyle: Postconditions
        debug_assert_eq!(
            result.len(),
            expected_hex_len,
            "Postcondition violated: ACL GENPASS output length must be 2*ceil(bits/8)"
        );
        debug_assert!(
            result.chars().all(|c| c.is_ascii_hexdigit()),
            "Postcondition violated: ACL GENPASS output must be valid hex"
        );

        RespValue::BulkString(Some(result.into_bytes()))
    }
}
