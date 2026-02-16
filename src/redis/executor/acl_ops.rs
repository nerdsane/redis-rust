//! ACL command implementations for CommandExecutor.
//!
//! Most ACL commands (AUTH, WHOAMI, LIST, USERS, GETUSER, SETUSER, DELUSER)
//! are handled at the connection level in `connection_optimized.rs` because
//! they require per-connection state (authenticated user, ACL manager).
//!
//! If these commands reach the executor, it means something is misconfigured.
//! We return clear errors and fire debug_assert in debug builds.
//!
//! ACL CAT and ACL GENPASS are stateless and can be handled here.
//!
//! # TigerStyle Invariants
//!
//! - ACL GENPASS output length matches requested bits/4 (hex encoding)

use super::CommandExecutor;
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_auth(&self) -> RespValue {
        // AUTH must be handled at the connection level (connection_optimized.rs).
        // If we get here, the command was routed incorrectly.
        debug_assert!(
            false,
            "AUTH reached executor — must be handled at connection level"
        );
        RespValue::err("ERR AUTH is handled at connection level, not executor")
    }

    pub(super) fn execute_acl_whoami(&self) -> RespValue {
        // ACL WHOAMI must be handled at the connection level — it needs the
        // per-connection authenticated_user.
        debug_assert!(
            false,
            "ACL WHOAMI reached executor — must be handled at connection level"
        );
        RespValue::err("ERR ACL WHOAMI is handled at connection level, not executor")
    }

    pub(super) fn execute_acl_list(&self) -> RespValue {
        // ACL LIST must be handled at the connection level — it needs the AclManager.
        debug_assert!(
            false,
            "ACL LIST reached executor — must be handled at connection level"
        );
        RespValue::err("ERR ACL LIST is handled at connection level, not executor")
    }

    pub(super) fn execute_acl_users(&self) -> RespValue {
        // ACL USERS must be handled at the connection level — it needs the AclManager.
        debug_assert!(
            false,
            "ACL USERS reached executor — must be handled at connection level"
        );
        RespValue::err("ERR ACL USERS is handled at connection level, not executor")
    }

    pub(super) fn execute_acl_getuser(&self, _username: &str) -> RespValue {
        // ACL GETUSER must be handled at the connection level — it needs the AclManager.
        debug_assert!(
            false,
            "ACL GETUSER reached executor — must be handled at connection level"
        );
        RespValue::err("ERR ACL GETUSER is handled at connection level, not executor")
    }

    pub(super) fn execute_acl_setuser(&self) -> RespValue {
        // ACL SETUSER must be handled at the connection level — it needs the AclManager.
        debug_assert!(
            false,
            "ACL SETUSER reached executor — must be handled at connection level"
        );
        RespValue::err("ERR ACL SETUSER is handled at connection level, not executor")
    }

    pub(super) fn execute_acl_deluser(&self) -> RespValue {
        // ACL DELUSER must be handled at the connection level — it needs the AclManager.
        debug_assert!(
            false,
            "ACL DELUSER reached executor — must be handled at connection level"
        );
        RespValue::err("ERR ACL DELUSER is handled at connection level, not executor")
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
