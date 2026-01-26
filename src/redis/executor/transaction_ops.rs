//! Transaction command implementations for CommandExecutor.
//!
//! Handles: MULTI, EXEC, DISCARD, WATCH, UNWATCH

use super::CommandExecutor;
use crate::redis::resp::RespValue;

impl CommandExecutor {
    pub(super) fn execute_multi(&mut self) -> RespValue {
        if self.in_transaction {
            return RespValue::err("ERR MULTI calls can not be nested");
        }
        self.in_transaction = true;
        self.queued_commands.clear();
        RespValue::simple("OK")
    }

    pub(super) fn execute_exec(&mut self) -> RespValue {
        if !self.in_transaction {
            return RespValue::err("ERR EXEC without MULTI");
        }

        // Check if any watched keys have changed
        let watch_violated = self.watched_keys.iter().any(|(key, original_value)| {
            let current_value = self.data.get(key).cloned();
            &current_value != original_value
        });

        // Clear transaction state
        self.in_transaction = false;
        let commands = std::mem::take(&mut self.queued_commands);
        self.watched_keys.clear();

        if watch_violated {
            // WATCH detected a change, abort the transaction
            return RespValue::BulkString(None);
        }

        // Execute all queued commands
        let results: Vec<RespValue> = commands.into_iter().map(|cmd| self.execute(&cmd)).collect();

        RespValue::Array(Some(results))
    }

    pub(super) fn execute_discard(&mut self) -> RespValue {
        if !self.in_transaction {
            return RespValue::err("ERR DISCARD without MULTI");
        }
        self.in_transaction = false;
        self.queued_commands.clear();
        self.watched_keys.clear();
        RespValue::simple("OK")
    }

    pub(super) fn execute_watch(&mut self, keys: &[String]) -> RespValue {
        if self.in_transaction {
            return RespValue::err("ERR WATCH inside MULTI is not allowed");
        }
        // Store current values of watched keys
        for key in keys {
            let current_value = self.data.get(key).cloned();
            self.watched_keys.insert(key.clone(), current_value);
        }
        RespValue::simple("OK")
    }

    pub(super) fn execute_unwatch(&mut self) -> RespValue {
        self.watched_keys.clear();
        RespValue::simple("OK")
    }
}
