//! Lua scripting support for Redis EVAL/EVALSHA commands.
//!
//! This module provides:
//! - Script caching via SHA1 for EVALSHA
//! - The actual Lua execution is in commands.rs execute_lua_script method
//!
//! TigerStyle: All functions have precondition/postcondition assertions.

use ahash::AHashMap;
use sha1::{Digest, Sha1};

/// Script cache for EVALSHA - maps SHA1 -> script source
#[derive(Debug, Default)]
pub struct ScriptCache {
    scripts: AHashMap<String, String>,
}

impl ScriptCache {
    pub fn new() -> Self {
        Self {
            scripts: AHashMap::new(),
        }
    }

    /// Compute SHA1 hash of a script
    pub fn compute_sha1(script: &str) -> String {
        let mut hasher = Sha1::new();
        hasher.update(script.as_bytes());
        let result = hasher.finalize();
        hex_encode(&result)
    }

    /// Cache a script and return its SHA1
    pub fn cache_script(&mut self, script: &str) -> String {
        debug_assert!(!script.is_empty(), "Precondition: script must not be empty");

        let sha1 = Self::compute_sha1(script);
        self.scripts.insert(sha1.clone(), script.to_string());

        debug_assert!(
            self.scripts.contains_key(&sha1),
            "Postcondition: script must be cached"
        );
        sha1
    }

    /// Get a script by SHA1
    pub fn get_script(&self, sha1: &str) -> Option<&String> {
        self.scripts.get(sha1)
    }

    /// Check if a script exists
    pub fn has_script(&self, sha1: &str) -> bool {
        self.scripts.contains_key(sha1)
    }

    /// Clear all cached scripts
    pub fn flush(&mut self) {
        self.scripts.clear();
    }
}

/// Simple hex encoding (avoid external dependency)
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_cache_sha1() {
        let script = "return 1";
        let sha1 = ScriptCache::compute_sha1(script);
        assert_eq!(sha1.len(), 40); // SHA1 produces 40 hex chars
    }

    #[test]
    fn test_script_cache_sha1_consistency() {
        let script = "return KEYS[1]";
        let sha1_1 = ScriptCache::compute_sha1(script);
        let sha1_2 = ScriptCache::compute_sha1(script);
        assert_eq!(sha1_1, sha1_2, "SHA1 must be deterministic");
    }

    #[test]
    fn test_script_cache_store_retrieve() {
        let mut cache = ScriptCache::new();
        let script = "return KEYS[1]";
        let sha1 = cache.cache_script(script);

        assert!(cache.has_script(&sha1));
        assert_eq!(cache.get_script(&sha1), Some(&script.to_string()));
    }

    #[test]
    fn test_script_cache_flush() {
        let mut cache = ScriptCache::new();
        let sha1 = cache.cache_script("return 1");

        assert!(cache.has_script(&sha1));
        cache.flush();
        assert!(!cache.has_script(&sha1));
    }

    #[test]
    fn test_script_cache_multiple_scripts() {
        let mut cache = ScriptCache::new();

        let sha1_1 = cache.cache_script("return 1");
        let sha1_2 = cache.cache_script("return 2");
        let sha1_3 = cache.cache_script("return KEYS[1]");

        assert!(cache.has_script(&sha1_1));
        assert!(cache.has_script(&sha1_2));
        assert!(cache.has_script(&sha1_3));

        assert_ne!(sha1_1, sha1_2);
        assert_ne!(sha1_1, sha1_3);
        assert_ne!(sha1_2, sha1_3);
    }
}
