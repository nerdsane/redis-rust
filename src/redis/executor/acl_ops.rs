//! ACL command implementations for CommandExecutor.
//!
//! Handles: AUTH, ACL WHOAMI, ACL LIST, ACL USERS, ACL GETUSER, ACL SETUSER,
//! ACL DELUSER, ACL CAT, ACL GENPASS

use super::CommandExecutor;
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_auth(&self) -> RespValue {
        // AUTH is handled at the connection level
        // If we get here, it means the server doesn't have ACL enabled
        RespValue::simple("OK")
    }

    pub(super) fn execute_acl_whoami(&self) -> RespValue {
        // Without ACL feature, default user is always authenticated
        RespValue::BulkString(Some(b"default".to_vec()))
    }

    pub(super) fn execute_acl_list(&self) -> RespValue {
        // Return default user rule
        let rule = "user default on nopass ~* +@all".to_string();
        RespValue::Array(Some(vec![RespValue::BulkString(Some(rule.into_bytes()))]))
    }

    pub(super) fn execute_acl_users(&self) -> RespValue {
        RespValue::Array(Some(vec![RespValue::BulkString(Some(b"default".to_vec()))]))
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
        let bytes = (bits as usize).div_ceil(8);
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
