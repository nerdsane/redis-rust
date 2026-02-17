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

        // Check existing value type
        if let Some(existing) = self.get_value(key) {
            if !matches!(existing, Value::String(_)) {
                return RespValue::err(
                    "WRONGTYPE Operation against a key holding the wrong kind of value",
                );
            }
        }

        // Ensure key exists as string and is large enough
        let need_create = self.get_value(key).is_none();
        if need_create {
            let required_len = byte_index.checked_add(1).expect("byte_index + 1 overflow");
            let sds = SDS::new(vec![0u8; required_len]);
            self.data.insert(key.to_string(), Value::String(sds));
            self.access_times
                .insert(key.to_string(), self.current_time);
        }

        // Get mutable reference to the string
        let sds = match self.data.get_mut(key) {
            Some(Value::String(s)) => s,
            _ => unreachable!("We just ensured it's a string"),
        };

        // Resize if needed
        let required_len = byte_index.checked_add(1).expect("byte_index + 1 overflow");
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
