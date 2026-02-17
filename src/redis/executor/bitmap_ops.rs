//! Bitmap command implementations for CommandExecutor.
//!
//! Handles: SETBIT, GETBIT
//!
//! Redis bitmaps are not a separate data type — they operate on string values
//! at the bit level. Bit ordering is big-endian within each byte:
//! bit 0 = MSB (0x80), bit 7 = LSB (0x01).

use super::CommandExecutor;
use crate::redis::data::{Value, SDS};
use crate::redis::resp::RespValue;

/// Maximum bit offset: 512MB * 8 bits = 2^32 bits
const MAX_BIT_OFFSET: u64 = 512 * 1024 * 1024 * 8;

impl CommandExecutor {
    pub(super) fn execute_setbit(&mut self, key: &str, offset: u64, value: u8) -> RespValue {
        debug_assert!(value <= 1, "Precondition: bit value must be 0 or 1");

        if offset >= MAX_BIT_OFFSET {
            return RespValue::err("ERR bit offset is not an integer or out of range");
        }

        let byte_index = (offset / 8) as usize;
        let bit_mask: u8 = 0x80 >> (offset % 8);

        // offset < MAX_BIT_OFFSET guarantees byte_index <= 536_870_911,
        // so byte_index + 1 cannot overflow usize on any supported platform.
        let required_len = match byte_index.checked_add(1) {
            Some(n) => n,
            None => return RespValue::err("ERR bit offset is not an integer or out of range"),
        };

        // Check existing value type and whether key exists in a single lookup
        let (need_create, is_wrong_type) = match self.get_value(key) {
            Some(Value::String(_)) => (false, false),
            Some(_) => (false, true),
            None => (true, false),
        };

        if is_wrong_type {
            return RespValue::err(
                "WRONGTYPE Operation against a key holding the wrong kind of value",
            );
        }

        if need_create {
            let sds = SDS::new(vec![0u8; required_len]);
            self.data.insert(key.to_string(), Value::String(sds));
            self.access_times
                .insert(key.to_string(), self.current_time);
        }

        // Get mutable reference to the string
        let sds = match self.data.get_mut(key) {
            Some(Value::String(s)) => s,
            // We just verified the key is a string (or created it as one)
            _ => {
                return RespValue::err("ERR internal error: expected string value");
            }
        };

        // Resize if needed
        if sds.len() < required_len {
            sds.resize(required_len);
        }

        let bytes = sds.as_bytes_mut();

        // Read old bit
        let old_bit = if bytes[byte_index] & bit_mask != 0 {
            1i64
        } else {
            0i64
        };

        // Set/clear bit
        if value == 1 {
            bytes[byte_index] |= bit_mask;
        } else {
            bytes[byte_index] &= !bit_mask;
        }

        // TigerStyle: Postcondition — verify the bit is now set correctly
        #[cfg(debug_assertions)]
        {
            let current_bit = if bytes[byte_index] & bit_mask != 0 {
                1u8
            } else {
                0u8
            };
            debug_assert_eq!(
                current_bit, value,
                "Postcondition: bit at offset {} must equal requested value {}",
                offset, value
            );
        }

        RespValue::Integer(old_bit)
    }

    pub(super) fn execute_getbit(&mut self, key: &str, offset: u64) -> RespValue {
        if offset >= MAX_BIT_OFFSET {
            return RespValue::err("ERR bit offset is not an integer or out of range");
        }

        let byte_index = (offset / 8) as usize;
        let bit_mask: u8 = 0x80 >> (offset % 8);

        match self.get_value(key) {
            None => RespValue::Integer(0),
            Some(Value::String(s)) => {
                if byte_index >= s.len() {
                    return RespValue::Integer(0);
                }
                let bit_value = if s.as_bytes()[byte_index] & bit_mask != 0 {
                    1i64
                } else {
                    0i64
                };
                RespValue::Integer(bit_value)
            }
            Some(_) => RespValue::err(
                "WRONGTYPE Operation against a key holding the wrong kind of value",
            ),
        }
    }
}
