//! Transaction command implementations for CommandExecutor.
//!
//! Handles: MULTI, EXEC, DISCARD, WATCH, UNWATCH
//!
//! # TigerStyle Invariants
//!
//! - `in_transaction` and `queued_commands` are always in sync:
//!   - If `in_transaction == false`, then `queued_commands.is_empty()`
//! - `watched_keys` is cleared when transaction ends (EXEC/DISCARD)

use super::CommandExecutor;
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_multi(&mut self) -> RespValue {
        // TigerStyle: Precondition - not already in transaction
        // (This is enforced by returning an error, which is correct Redis behavior)
        if self.in_transaction {
            return RespValue::err("ERR MULTI calls can not be nested");
        }

        self.in_transaction = true;
        self.queued_commands.clear();

        // TigerStyle: Postconditions
        debug_assert!(
            self.in_transaction,
            "Postcondition violated: in_transaction must be true after MULTI"
        );
        debug_assert!(
            self.queued_commands.is_empty(),
            "Postcondition violated: queued_commands must be empty after MULTI"
        );

        RespValue::simple("OK")
    }

    pub(super) fn execute_exec(&mut self) -> RespValue {
        // TigerStyle: Precondition - must be in transaction
        if !self.in_transaction {
            return RespValue::err("ERR EXEC without MULTI");
        }

        // TigerStyle: Capture pre-state for postcondition verification
        #[cfg(debug_assertions)]
        let queued_count = self.queued_commands.len();

        // Check if any watched keys have changed
        let watch_violated = self.watched_keys.iter().any(|(key, original_value)| {
            let current_value = self.data.get(key).cloned();
            &current_value != original_value
        });

        // Clear transaction state
        self.in_transaction = false;
        let commands = std::mem::take(&mut self.queued_commands);
        self.watched_keys.clear();

        // TigerStyle: Postconditions - transaction state must be reset
        debug_assert!(
            !self.in_transaction,
            "Postcondition violated: in_transaction must be false after EXEC"
        );
        debug_assert!(
            self.queued_commands.is_empty(),
            "Postcondition violated: queued_commands must be empty after EXEC"
        );
        debug_assert!(
            self.watched_keys.is_empty(),
            "Postcondition violated: watched_keys must be empty after EXEC"
        );

        if watch_violated {
            // WATCH detected a change, abort the transaction
            return RespValue::BulkString(None);
        }

        // Execute all queued commands
        let results: Vec<RespValue> = commands.into_iter().map(|cmd| self.execute(&cmd)).collect();

        // TigerStyle: Postcondition - results count must equal queued count
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            results.len(),
            queued_count,
            "Postcondition violated: EXEC must return one result per queued command"
        );

        RespValue::Array(Some(results))
    }

    pub(super) fn execute_discard(&mut self) -> RespValue {
        // TigerStyle: Precondition - must be in transaction
        if !self.in_transaction {
            return RespValue::err("ERR DISCARD without MULTI");
        }

        self.in_transaction = false;
        self.queued_commands.clear();
        self.watched_keys.clear();

        // TigerStyle: Postconditions - all transaction state must be reset
        debug_assert!(
            !self.in_transaction,
            "Postcondition violated: in_transaction must be false after DISCARD"
        );
        debug_assert!(
            self.queued_commands.is_empty(),
            "Postcondition violated: queued_commands must be empty after DISCARD"
        );
        debug_assert!(
            self.watched_keys.is_empty(),
            "Postcondition violated: watched_keys must be empty after DISCARD"
        );

        RespValue::simple("OK")
    }

    pub(super) fn execute_watch(&mut self, keys: &[String]) -> RespValue {
        // TigerStyle: Precondition - cannot WATCH inside a transaction
        if self.in_transaction {
            return RespValue::err("ERR WATCH inside MULTI is not allowed");
        }

        // TigerStyle: Capture pre-state
        #[cfg(debug_assertions)]
        let pre_watch_count = self.watched_keys.len();

        // Store current values of watched keys
        for key in keys {
            let current_value = self.data.get(key).cloned();
            self.watched_keys.insert(key.clone(), current_value);
        }

        // TigerStyle: Postcondition - all requested keys must be watched
        #[cfg(debug_assertions)]
        {
            for key in keys {
                debug_assert!(
                    self.watched_keys.contains_key(key),
                    "Postcondition violated: WATCH must add all requested keys"
                );
            }
            // Watch count should have increased (unless keys were already watched)
            debug_assert!(
                self.watched_keys.len() >= pre_watch_count,
                "Postcondition violated: WATCH must not decrease watched key count"
            );
        }

        RespValue::simple("OK")
    }

    pub(super) fn execute_unwatch(&mut self) -> RespValue {
        self.watched_keys.clear();

        // TigerStyle: Postcondition - all watches must be cleared
        debug_assert!(
            self.watched_keys.is_empty(),
            "Postcondition violated: watched_keys must be empty after UNWATCH"
        );

        RespValue::simple("OK")
    }
}
