//! CONFIG command implementation.
//!
//! Provides CONFIG GET (with glob matching), CONFIG SET, and CONFIG RESETSTAT.
//! The `ServerConfig` struct holds a map of configuration parameters seeded with
//! Redis 7 defaults for the ~40 parameters the official Tcl test suite requires.

use super::CommandExecutor;
use crate::redis::resp::RespValue;
use ahash::AHashMap;

/// Server configuration store for CONFIG GET/SET.
pub struct ServerConfig {
    params: AHashMap<String, String>,
}

impl ServerConfig {
    pub fn new() -> Self {
        let mut params = AHashMap::new();

        // Encoding thresholds
        params.insert("list-max-listpack-size".into(), "-2".into());
        params.insert("list-compress-depth".into(), "0".into());
        params.insert("set-max-listpack-entries".into(), "128".into());
        params.insert("set-max-intset-entries".into(), "512".into());
        params.insert("hash-max-listpack-entries".into(), "128".into());
        params.insert("hash-max-listpack-value".into(), "64".into());
        params.insert("zset-max-listpack-entries".into(), "128".into());
        params.insert("zset-max-listpack-value".into(), "64".into());

        // Memory
        params.insert("maxmemory".into(), "0".into());
        params.insert("maxmemory-policy".into(), "noeviction".into());
        params.insert("active-expire-enabled".into(), "yes".into());

        // Persistence
        params.insert("save".into(), "".into());
        params.insert("appendonly".into(), "no".into());
        params.insert("rdbcompression".into(), "yes".into());

        // Networking
        params.insert("hz".into(), "10".into());
        params.insert("dynamic-hz".into(), "yes".into());
        params.insert("timeout".into(), "0".into());
        params.insert("tcp-keepalive".into(), "300".into());
        params.insert("maxclients".into(), "10000".into());

        // Limits
        params.insert("proto-max-bulk-len".into(), "512000000".into());
        params.insert("client-query-buffer-limit".into(), "1073741824".into());

        // Scripting
        params.insert("lua-time-limit".into(), "5000".into());

        // Lazy free
        params.insert("lazyfree-lazy-eviction".into(), "no".into());
        params.insert("lazyfree-lazy-expire".into(), "no".into());
        params.insert("lazyfree-lazy-server-del".into(), "no".into());

        // Replication
        params.insert("min-replicas-to-write".into(), "0".into());
        params.insert("replica-serve-stale-data".into(), "yes".into());
        params.insert("replica-read-only".into(), "yes".into());

        // Additional params the Tcl harness commonly reads
        params.insert("bind".into(), "".into());
        params.insert("port".into(), "6379".into());
        params.insert("databases".into(), "16".into());
        params.insert("loglevel".into(), "notice".into());
        params.insert("logfile".into(), "".into());
        params.insert("dir".into(), ".".into());
        params.insert("dbfilename".into(), "dump.rdb".into());
        params.insert("requirepass".into(), "".into());
        params.insert("activedefrag".into(), "no".into());
        params.insert("no-appendfsync-on-rewrite".into(), "no".into());
        params.insert("slave-lazy-flush".into(), "no".into());
        params.insert("tracking-table-max-keys".into(), "0".into());
        params.insert("close-on-oom".into(), "no".into());
        params.insert("repl-min-slaves-to-write".into(), "0".into());
        params.insert("latency-tracking".into(), "yes".into());
        params.insert("close-files-after-invoked-defer".into(), "no".into());
        params.insert("slowlog-log-slower-than".into(), "10000".into());
        params.insert("slowlog-max-len".into(), "128".into());
        params.insert("list-max-ziplist-size".into(), "-2".into());
        params.insert("lfu-log-factor".into(), "10".into());
        params.insert("lfu-decay-time".into(), "1".into());

        let config = ServerConfig { params };

        #[cfg(debug_assertions)]
        config.verify_invariants();

        config
    }

    /// Return all (key, value) pairs whose key matches the given glob pattern.
    pub fn get_matching(&self, pattern: &str) -> Vec<(&str, &str)> {
        self.params
            .iter()
            .filter(|(k, _)| glob_match(k.as_bytes(), pattern.as_bytes()))
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    /// Upsert a configuration parameter.
    pub fn set(&mut self, param: &str, value: &str) {
        self.params
            .insert(param.to_lowercase(), value.to_string());
    }

    #[cfg(debug_assertions)]
    pub(crate) fn verify_invariants(&self) {
        for key in self.params.keys() {
            debug_assert!(!key.is_empty(), "Config key must not be empty");
        }
    }
}

// ============================================================================
// Glob matching (standalone, no &self needed)
// ============================================================================

fn glob_match(text: &[u8], pattern: &[u8]) -> bool {
    glob_match_inner(text, pattern, 0, 0)
}

fn glob_match_inner(text: &[u8], pattern: &[u8], t: usize, p: usize) -> bool {
    if p >= pattern.len() {
        return t >= text.len();
    }
    match pattern[p] {
        b'*' => {
            for i in t..=text.len() {
                if glob_match_inner(text, pattern, i, p + 1) {
                    return true;
                }
            }
            false
        }
        b'?' => {
            if t >= text.len() {
                false
            } else {
                glob_match_inner(text, pattern, t + 1, p + 1)
            }
        }
        b'[' => {
            let mut end = p + 1;
            while end < pattern.len() && pattern[end] != b']' {
                end += 1;
            }
            if end >= pattern.len() || t >= text.len() {
                return false;
            }
            let class = &pattern[p + 1..end];
            let (negate, class) = if !class.is_empty() && class[0] == b'^' {
                (true, &class[1..])
            } else {
                (false, class)
            };
            let mut matched = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == b'-' {
                    if text[t] >= class[i] && text[t] <= class[i + 2] {
                        matched = true;
                    }
                    i += 3;
                } else {
                    if class[i] == text[t] {
                        matched = true;
                    }
                    i += 1;
                }
            }
            if negate {
                matched = !matched;
            }
            if matched {
                glob_match_inner(text, pattern, t + 1, end + 1)
            } else {
                false
            }
        }
        ch => {
            if t >= text.len() || text[t] != ch {
                false
            } else {
                glob_match_inner(text, pattern, t + 1, p + 1)
            }
        }
    }
}

// ============================================================================
// Executor methods
// ============================================================================

impl CommandExecutor {
    pub(super) fn execute_config_get(&self, pattern: &str) -> RespValue {
        debug_assert!(
            !pattern.is_empty(),
            "Precondition: CONFIG GET pattern must not be empty"
        );

        let matches = self.config.get_matching(pattern);
        let mut result = Vec::with_capacity(matches.len() * 2);
        for (k, v) in &matches {
            result.push(RespValue::BulkString(Some(k.as_bytes().to_vec())));
            result.push(RespValue::BulkString(Some(v.as_bytes().to_vec())));
        }

        debug_assert!(
            result.len() % 2 == 0,
            "Postcondition: CONFIG GET must return even number of elements"
        );
        RespValue::Array(Some(result))
    }

    pub(super) fn execute_config_set(&mut self, param: &str, value: &str) -> RespValue {
        debug_assert!(
            !param.is_empty(),
            "Precondition: CONFIG SET param must not be empty"
        );
        self.config.set(param, value);

        #[cfg(debug_assertions)]
        self.config.verify_invariants();

        RespValue::ok()
    }

    pub(super) fn execute_config_resetstat(&mut self) -> RespValue {
        self.commands_processed = 0;
        debug_assert!(
            self.commands_processed == 0,
            "Postcondition: commands_processed must be 0"
        );
        RespValue::ok()
    }
}
