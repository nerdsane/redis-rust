//! Simple Dynamic String with Small String Optimization (SSO)
//!
//! Strings ≤23 bytes are stored inline without heap allocation.
//! This matches Redis's approach and significantly reduces allocator pressure
//! since most Redis keys are short (e.g., "user:123", "session:abc").

use serde::{Deserialize, Serialize};

/// Small String Optimization threshold - strings up to this size are stored inline
const SSO_MAX_LEN: usize = 23;

/// Simple Dynamic String with Small String Optimization (SSO)
#[derive(Clone, Debug)]
pub enum SDS {
    /// Inline storage for small strings (no heap allocation)
    Inline { len: u8, data: [u8; SSO_MAX_LEN] },
    /// Heap storage for larger strings
    Heap(Vec<u8>),
}

impl SDS {
    /// Verify all invariants hold for this SDS
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        match self {
            SDS::Inline { len, data: _ } => {
                // Invariant 1: Inline len must be <= SSO_MAX_LEN
                debug_assert!(
                    (*len as usize) <= SSO_MAX_LEN,
                    "Invariant violated: inline len {} exceeds SSO_MAX_LEN {}",
                    len,
                    SSO_MAX_LEN
                );
                // Invariant 2: len() must be consistent
                debug_assert_eq!(
                    self.len(),
                    *len as usize,
                    "Invariant violated: len() must equal stored len"
                );
            }
            SDS::Heap(_data) => {
                // Invariant: Heap strings should be > SSO_MAX_LEN
                // (unless created via append that didn't optimize)
                // This is a soft invariant - we don't enforce it for simplicity
            }
        }

        // Invariant 3: is_empty() iff len() == 0
        debug_assert_eq!(
            self.is_empty(),
            self.len() == 0,
            "Invariant violated: is_empty() must equal len() == 0"
        );

        // Invariant 4: as_bytes().len() must equal len()
        debug_assert_eq!(
            self.as_bytes().len(),
            self.len(),
            "Invariant violated: as_bytes().len() must equal len()"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn verify_invariants(&self) {}

    /// Create SDS from bytes, using inline storage for small strings
    #[inline]
    pub fn new(data: Vec<u8>) -> Self {
        let sds = if data.len() <= SSO_MAX_LEN {
            let mut inline_data = [0u8; SSO_MAX_LEN];
            inline_data[..data.len()].copy_from_slice(&data);
            SDS::Inline {
                len: data.len() as u8,
                data: inline_data,
            }
        } else {
            SDS::Heap(data)
        };

        // TigerStyle: Postcondition - verify construction succeeded
        sds.verify_invariants();
        sds
    }

    /// Create SDS from string slice, using inline storage for small strings
    #[inline]
    pub fn from_str(s: &str) -> Self {
        let bytes = s.as_bytes();
        let sds = if bytes.len() <= SSO_MAX_LEN {
            let mut inline_data = [0u8; SSO_MAX_LEN];
            inline_data[..bytes.len()].copy_from_slice(bytes);
            SDS::Inline {
                len: bytes.len() as u8,
                data: inline_data,
            }
        } else {
            SDS::Heap(bytes.to_vec())
        };

        // TigerStyle: Postconditions
        debug_assert_eq!(
            sds.len(),
            s.len(),
            "Postcondition violated: SDS len must equal source string len"
        );

        sds.verify_invariants();
        sds
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            SDS::Inline { len, .. } => *len as usize,
            SDS::Heap(data) => data.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            SDS::Inline { len, data } => &data[..*len as usize],
            SDS::Heap(data) => data,
        }
    }

    pub fn to_string(&self) -> String {
        String::from_utf8_lossy(self.as_bytes()).to_string()
    }

    pub fn append(&mut self, other: &SDS) {
        // TigerStyle: Preconditions - capture state for postcondition check
        #[cfg(debug_assertions)]
        let pre_len = self.len();
        #[cfg(debug_assertions)]
        let other_len = other.len();

        let new_len = self.len() + other.len();

        // If result fits in inline, stay inline; otherwise convert to heap
        if new_len <= SSO_MAX_LEN {
            // Can stay inline
            match self {
                SDS::Inline { len, data } => {
                    let current_len = *len as usize;
                    data[current_len..current_len + other.len()].copy_from_slice(other.as_bytes());
                    *len = new_len as u8;
                }
                SDS::Heap(data) => {
                    // Heap but small enough - just extend
                    data.extend_from_slice(other.as_bytes());
                }
            }
        } else {
            // Need heap storage
            let mut new_data = Vec::with_capacity(new_len);
            new_data.extend_from_slice(self.as_bytes());
            new_data.extend_from_slice(other.as_bytes());
            *self = SDS::Heap(new_data);
        }

        // TigerStyle: Postconditions
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.len(),
                pre_len + other_len,
                "Postcondition violated: len must equal pre_len + other_len after append"
            );
        }

        self.verify_invariants();
    }
}

impl PartialEq for SDS {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for SDS {}

impl std::hash::Hash for SDS {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

// Custom serialization to store as bytes (backwards compatible)
impl Serialize for SDS {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(self.as_bytes())
    }
}

impl<'de> Deserialize<'de> for SDS {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        Ok(SDS::new(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sds_new() {
        let data = vec![104, 101, 108, 108, 111]; // "hello"
        let sds = SDS::new(data.clone());

        assert_eq!(sds.len(), 5);
        assert!(!sds.is_empty());
        assert_eq!(sds.as_bytes(), &data);
        assert_eq!(sds.to_string(), "hello");
    }

    #[test]
    fn test_sds_from_str() {
        let sds = SDS::from_str("world");

        assert_eq!(sds.len(), 5);
        assert!(!sds.is_empty());
        assert_eq!(sds.as_bytes(), b"world");
        assert_eq!(sds.to_string(), "world");
    }

    #[test]
    fn test_sds_empty() {
        let sds = SDS::from_str("");

        assert_eq!(sds.len(), 0);
        assert!(sds.is_empty());
        assert_eq!(sds.as_bytes(), b"");
        assert_eq!(sds.to_string(), "");
    }

    #[test]
    fn test_sds_append() {
        let mut sds = SDS::from_str("hello");
        let other = SDS::from_str(" world");

        sds.append(&other);

        assert_eq!(sds.len(), 11);
        assert_eq!(sds.to_string(), "hello world");
    }

    #[test]
    fn test_sds_append_empty() {
        let mut sds = SDS::from_str("test");
        let empty = SDS::from_str("");

        sds.append(&empty);

        assert_eq!(sds.len(), 4);
        assert_eq!(sds.to_string(), "test");
    }

    #[test]
    fn test_sds_append_to_empty() {
        let mut sds = SDS::from_str("");
        let other = SDS::from_str("data");

        sds.append(&other);

        assert_eq!(sds.len(), 4);
        assert_eq!(sds.to_string(), "data");
    }

    #[test]
    fn test_sds_binary_data() {
        // Test with binary data including null bytes
        let data = vec![0, 1, 2, 0, 255];
        let sds = SDS::new(data.clone());

        assert_eq!(sds.len(), 5);
        assert_eq!(sds.as_bytes(), &data);
    }

    #[test]
    fn test_sds_invariants_maintained() {
        // Test invariants through various operations
        let mut sds = SDS::from_str("");
        assert!(sds.is_empty());
        assert_eq!(sds.len(), 0);

        // Append to empty
        sds.append(&SDS::from_str("a"));
        assert!(!sds.is_empty());
        assert_eq!(sds.len(), 1);

        // Multiple appends
        sds.append(&SDS::from_str("bc"));
        assert_eq!(sds.len(), 3);

        sds.append(&SDS::from_str("def"));
        assert_eq!(sds.len(), 6);
        assert_eq!(sds.to_string(), "abcdef");
    }

    // === Small String Optimization (SSO) Tests ===

    #[test]
    fn test_sso_small_string_is_inline() {
        // Strings <= 23 bytes should use inline storage
        let sds = SDS::from_str("short");
        assert_eq!(sds.len(), 5);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 5),
            SDS::Heap(_) => panic!("Expected inline storage for short string"),
        }
    }

    #[test]
    fn test_sso_max_inline_string() {
        // 23 bytes exactly should still be inline
        let s = "12345678901234567890123"; // 23 chars
        assert_eq!(s.len(), 23);
        let sds = SDS::from_str(s);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 23),
            SDS::Heap(_) => panic!("Expected inline storage for 23-byte string"),
        }
        assert_eq!(sds.to_string(), s);
    }

    #[test]
    fn test_sso_large_string_is_heap() {
        // 24+ bytes should use heap storage
        let s = "123456789012345678901234"; // 24 chars
        assert_eq!(s.len(), 24);
        let sds = SDS::from_str(s);
        match &sds {
            SDS::Inline { .. } => panic!("Expected heap storage for 24-byte string"),
            SDS::Heap(data) => assert_eq!(data.len(), 24),
        }
        assert_eq!(sds.to_string(), s);
    }

    #[test]
    fn test_sso_append_stays_inline() {
        // Append that keeps total <= 23 bytes should stay inline
        let mut sds = SDS::from_str("hello"); // 5 bytes
        sds.append(&SDS::from_str(" world")); // +6 = 11 bytes

        assert_eq!(sds.len(), 11);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 11),
            SDS::Heap(_) => panic!("Expected inline storage after append"),
        }
        assert_eq!(sds.to_string(), "hello world");
    }

    #[test]
    fn test_sso_append_transitions_to_heap() {
        // Append that exceeds 23 bytes should transition to heap
        let mut sds = SDS::from_str("12345678901234567890"); // 20 bytes
        sds.append(&SDS::from_str("12345")); // +5 = 25 bytes

        assert_eq!(sds.len(), 25);
        match &sds {
            SDS::Inline { .. } => panic!("Expected heap storage after append exceeds SSO"),
            SDS::Heap(data) => assert_eq!(data.len(), 25),
        }
        assert_eq!(sds.to_string(), "1234567890123456789012345");
    }

    #[test]
    fn test_sso_empty_string() {
        let sds = SDS::from_str("");
        assert!(sds.is_empty());
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 0),
            SDS::Heap(_) => panic!("Expected inline storage for empty string"),
        }
    }

    #[test]
    fn test_sso_new_from_vec() {
        // Small vec should be inline
        let sds = SDS::new(vec![1, 2, 3, 4, 5]);
        match &sds {
            SDS::Inline { len, .. } => assert_eq!(*len, 5),
            SDS::Heap(_) => panic!("Expected inline storage for small vec"),
        }

        // Large vec should be heap
        let sds = SDS::new(vec![0u8; 30]);
        match &sds {
            SDS::Inline { .. } => panic!("Expected heap storage for large vec"),
            SDS::Heap(data) => assert_eq!(data.len(), 30),
        }
    }

    #[test]
    fn test_sso_equality() {
        // Inline == Inline
        let a = SDS::from_str("hello");
        let b = SDS::from_str("hello");
        assert_eq!(a, b);

        // Different inline strings
        let c = SDS::from_str("world");
        assert_ne!(a, c);

        // Heap == Heap
        let d = SDS::from_str("this is a very long string that exceeds the SSO limit");
        let e = SDS::from_str("this is a very long string that exceeds the SSO limit");
        assert_eq!(d, e);
    }

    #[test]
    fn test_sso_typical_redis_keys() {
        // Most Redis keys are short - test typical patterns
        let keys = [
            "user:123",           // 8 bytes - inline
            "session:abc123",     // 14 bytes - inline
            "cache:homepage",     // 14 bytes - inline
            "counter",            // 7 bytes - inline
            "rate_limit:user:42", // 18 bytes - inline
        ];

        for key in &keys {
            let sds = SDS::from_str(key);
            assert!(key.len() <= 23, "Test key should be <= 23 bytes");
            match &sds {
                SDS::Inline { .. } => {} // Expected
                SDS::Heap(_) => panic!("Key '{}' should use inline storage", key),
            }
        }
    }
}
