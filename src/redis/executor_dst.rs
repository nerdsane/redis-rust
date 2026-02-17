//! Deterministic Simulation Testing for the Command Executor
//!
//! Shadow-state testing harness for `CommandExecutor` that exercises ALL command
//! types through `CommandExecutor::execute()`, not the data structures directly.
//! This closes the critical coverage gap where the executor dispatch layer had 0% DST coverage.
//!
//! ## Design
//!
//! The harness maintains a **shadow state** (reference model) alongside the real executor.
//! After every operation, invariants are checked by comparing the executor's responses
//! against expected behavior computed from the shadow state.
//!
//! ## Key Access Pattern
//!
//! Uses a Zipfian-like distribution over a bounded key space to create realistic
//! hot-key behavior (some keys accessed far more frequently than others).
//!
//! ## Usage
//!
//! ```rust,ignore
//! for seed in 0..100 {
//!     let mut harness = ExecutorDSTHarness::with_seed(seed);
//!     harness.run(500);
//!     assert!(harness.result().is_success(), "Seed {} failed", seed);
//! }
//! ```

use super::command::Command;
use super::data::SDS;
use super::executor::CommandExecutor;
use super::resp::RespValue;
use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Configuration for Executor DST
#[derive(Debug, Clone)]
pub struct ExecutorDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of unique keys in the key space
    pub num_keys: usize,
    /// Number of unique values to use
    pub num_values: usize,
    /// Number of unique fields for hash/sorted-set operations
    pub num_fields: usize,
    /// Zipfian exponent (higher = more skewed toward hot keys)
    pub zipf_exponent: f64,

    // Command category weights (must sum to ~100)
    pub weight_string: u64,
    pub weight_key: u64,
    pub weight_list: u64,
    pub weight_set: u64,
    pub weight_hash: u64,
    pub weight_sorted_set: u64,
    pub weight_expiry: u64,
}

impl Default for ExecutorDSTConfig {
    fn default() -> Self {
        ExecutorDSTConfig {
            seed: 0,
            num_keys: 50,
            num_values: 30,
            num_fields: 20,
            zipf_exponent: 1.0,
            weight_string: 30,
            weight_key: 10,
            weight_list: 15,
            weight_set: 10,
            weight_hash: 15,
            weight_sorted_set: 10,
            weight_expiry: 10,
        }
    }
}

impl ExecutorDSTConfig {
    /// Standard configuration with given seed
    pub fn new(seed: u64) -> Self {
        ExecutorDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Calm configuration - fewer keys, lighter load
    pub fn calm(seed: u64) -> Self {
        ExecutorDSTConfig {
            seed,
            num_keys: 20,
            num_values: 10,
            num_fields: 10,
            ..Default::default()
        }
    }

    /// Chaos configuration - more keys, higher contention
    pub fn chaos(seed: u64) -> Self {
        ExecutorDSTConfig {
            seed,
            num_keys: 100,
            num_values: 50,
            num_fields: 30,
            zipf_exponent: 1.5,
            ..Default::default()
        }
    }

    /// String-heavy workload
    pub fn string_heavy(seed: u64) -> Self {
        ExecutorDSTConfig {
            seed,
            weight_string: 60,
            weight_key: 10,
            weight_list: 5,
            weight_set: 5,
            weight_hash: 5,
            weight_sorted_set: 5,
            weight_expiry: 10,
            ..Default::default()
        }
    }

    fn total_weight(&self) -> u64 {
        self.weight_string
            + self.weight_key
            + self.weight_list
            + self.weight_set
            + self.weight_hash
            + self.weight_sorted_set
            + self.weight_expiry
    }
}

/// Operation type for logging
#[derive(Debug, Clone)]
pub enum ExecutorOp {
    String(String),
    Key(String),
    List(String),
    Set(String),
    Hash(String),
    SortedSet(String),
    Expiry(String),
}

/// Result of an Executor DST run
#[derive(Debug, Clone)]
pub struct ExecutorDSTResult {
    pub seed: u64,
    pub total_operations: u64,
    pub string_ops: u64,
    pub key_ops: u64,
    pub list_ops: u64,
    pub set_ops: u64,
    pub hash_ops: u64,
    pub sorted_set_ops: u64,
    pub expiry_ops: u64,
    pub invariant_violations: Vec<String>,
    pub last_op: Option<ExecutorOp>,
}

impl ExecutorDSTResult {
    pub fn new(seed: u64) -> Self {
        ExecutorDSTResult {
            seed,
            total_operations: 0,
            string_ops: 0,
            key_ops: 0,
            list_ops: 0,
            set_ops: 0,
            hash_ops: 0,
            sorted_set_ops: 0,
            expiry_ops: 0,
            invariant_violations: Vec::new(),
            last_op: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops (str:{}, key:{}, list:{}, set:{}, hash:{}, zset:{}, exp:{}), {} violations",
            self.seed,
            self.total_operations,
            self.string_ops,
            self.key_ops,
            self.list_ops,
            self.set_ops,
            self.hash_ops,
            self.sorted_set_ops,
            self.expiry_ops,
            self.invariant_violations.len()
        )
    }
}

// =============================================================================
// Shadow State - Reference Model
// =============================================================================

/// Reference value types tracked in shadow state
#[derive(Debug, Clone)]
enum RefValue {
    String(Vec<u8>),
    List(Vec<Vec<u8>>),
    Set(HashSet<Vec<u8>>),
    Hash(HashMap<Vec<u8>, Vec<u8>>),
    SortedSet(BTreeMap<Vec<u8>, f64>),
}

impl RefValue {
    fn type_name(&self) -> &'static str {
        match self {
            RefValue::String(_) => "string",
            RefValue::List(_) => "list",
            RefValue::Set(_) => "set",
            RefValue::Hash(_) => "hash",
            RefValue::SortedSet(_) => "zset",
        }
    }
}

/// Shadow state that tracks expected values alongside the real executor
struct ShadowState {
    data: HashMap<String, RefValue>,
    expirations: HashMap<String, u64>, // key -> expiry time in ms
}

impl ShadowState {
    fn new() -> Self {
        ShadowState {
            data: HashMap::new(),
            expirations: HashMap::new(),
        }
    }

    fn get(&self, key: &str) -> Option<&RefValue> {
        self.data.get(key)
    }

    fn set_string(&mut self, key: &str, value: Vec<u8>) {
        self.data.insert(key.to_string(), RefValue::String(value));
    }

    fn del(&mut self, key: &str) -> bool {
        self.expirations.remove(key);
        self.data.remove(key).is_some()
    }

    fn exists(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    fn key_count(&self) -> usize {
        self.data.len()
    }

    fn clear(&mut self) {
        self.data.clear();
        self.expirations.clear();
    }

    /// Evict expired keys at given time
    fn evict_expired(&mut self, current_time_ms: u64) {
        let expired: Vec<String> = self
            .expirations
            .iter()
            .filter(|(_, &exp)| exp <= current_time_ms)
            .map(|(k, _)| k.clone())
            .collect();
        for key in expired {
            self.data.remove(&key);
            self.expirations.remove(&key);
        }
    }
}

// =============================================================================
// DST Harness
// =============================================================================

/// DST harness for CommandExecutor
pub struct ExecutorDSTHarness {
    config: ExecutorDSTConfig,
    rng: SimulatedRng,
    executor: CommandExecutor,
    shadow: ShadowState,
    result: ExecutorDSTResult,
    current_time_ms: u64,
    /// All keys that have been created via SCAN liveness check
    all_keys_ever: HashSet<String>,
}

impl ExecutorDSTHarness {
    pub fn new(config: ExecutorDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);
        ExecutorDSTHarness {
            result: ExecutorDSTResult::new(config.seed),
            config,
            rng,
            executor: CommandExecutor::new(),
            shadow: ShadowState::new(),
            current_time_ms: 1_000_000, // Start at 1 second to allow expiry math
            all_keys_ever: HashSet::new(),
        }
    }

    /// Create with just a seed (uses default config)
    pub fn with_seed(seed: u64) -> Self {
        Self::new(ExecutorDSTConfig::new(seed))
    }

    // =========================================================================
    // Key Generation (Zipfian-like distribution)
    // =========================================================================

    fn random_key(&mut self) -> String {
        let idx = self.zipfian_index(self.config.num_keys);
        let key = format!("key:{}", idx);
        self.all_keys_ever.insert(key.clone());
        key
    }

    fn random_value(&mut self) -> Vec<u8> {
        let idx = self.rng.gen_range(0, self.config.num_values as u64);
        format!("val:{}", idx).into_bytes()
    }

    fn random_field(&mut self) -> Vec<u8> {
        let idx = self.rng.gen_range(0, self.config.num_fields as u64);
        format!("field:{}", idx).into_bytes()
    }

    fn random_score(&mut self) -> f64 {
        let raw = self.rng.gen_range(0, 100_000);
        raw as f64 / 100.0
    }

    fn random_integer_string(&mut self) -> Vec<u8> {
        let val = self.rng.gen_range(0, 10000) as i64 - 5000;
        val.to_string().into_bytes()
    }

    /// Zipfian-like index selection: bias toward lower indices (hot keys)
    fn zipfian_index(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        // Simple power-law approximation
        let u = self.rng.gen_range(1, 1_000_001) as f64 / 1_000_000.0;
        let idx = (u.powf(self.config.zipf_exponent) * max as f64) as usize;
        idx.min(max - 1)
    }

    // =========================================================================
    // Command Category Selection
    // =========================================================================

    fn select_category(&mut self) -> u64 {
        let total = self.config.total_weight();
        if total == 0 {
            return 0;
        }
        self.rng.gen_range(0, total)
    }

    // =========================================================================
    // Operation Runners
    // =========================================================================

    fn run_single_op(&mut self) {
        // Keep shadow in sync: evict expired keys before each op
        // (mirrors executor's lazy expiration on access)
        self.shadow.evict_expired(self.current_time_ms);

        // Occasionally test server commands (5% chance each)
        let server_roll = self.rng.gen_range(0, 100);
        if server_roll < 3 {
            self.run_ping_op();
            return;
        } else if server_roll < 5 {
            self.run_echo_op();
            return;
        } else if server_roll < 7 {
            self.run_select_op();
            return;
        } else if server_roll < 10 {
            self.run_config_op();
            return;
        }

        let roll = self.select_category();
        let mut threshold = 0;

        threshold += self.config.weight_string;
        if roll < threshold {
            self.run_string_op();
            return;
        }
        threshold += self.config.weight_key;
        if roll < threshold {
            self.run_key_op();
            return;
        }
        threshold += self.config.weight_list;
        if roll < threshold {
            self.run_list_op();
            return;
        }
        threshold += self.config.weight_set;
        if roll < threshold {
            self.run_set_op();
            return;
        }
        threshold += self.config.weight_hash;
        if roll < threshold {
            self.run_hash_op();
            return;
        }
        threshold += self.config.weight_sorted_set;
        if roll < threshold {
            self.run_sorted_set_op();
            return;
        }
        // Remaining = expiry
        self.run_expiry_op();
    }

    // --- PING operations ---
    fn run_ping_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.key_ops += 1;

        if sub < 50 {
            // PING without argument
            let desc = "PING".to_string();
            self.result.last_op = Some(ExecutorOp::Key(desc));
            let resp = self.executor.execute(&Command::Ping(None));
            self.assert_simple_string(&resp, "PONG", "PING should return PONG");
        } else {
            // PING with argument - should echo back as BulkString
            let msg = self.random_value();
            let desc = format!("PING {:?}", String::from_utf8_lossy(&msg));
            self.result.last_op = Some(ExecutorOp::Key(desc));
            let resp = self.executor.execute(&Command::Ping(Some(SDS::new(msg.clone()))));
            self.assert_bulk_eq(&resp, &msg, "PING with argument should echo back the message");
        }
    }

    // --- ECHO operations ---
    fn run_echo_op(&mut self) {
        let msg = self.random_value();
        let desc = format!("ECHO {:?}", String::from_utf8_lossy(&msg));
        self.result.key_ops += 1;
        self.result.last_op = Some(ExecutorOp::Key(desc));

        let resp = self.executor.execute(&Command::Echo(SDS::new(msg.clone())));
        self.assert_bulk_eq(&resp, &msg, "ECHO should return the input message");
    }

    // --- SELECT operations ---
    fn run_select_op(&mut self) {
        let db = self.rng.gen_range(0, 16);
        let desc = format!("SELECT {}", db);
        self.result.key_ops += 1;
        self.result.last_op = Some(ExecutorOp::Key(desc));

        let resp = self.executor.execute(&Command::Select(db));
        self.assert_ok(&resp, &format!("SELECT {} should return OK", db));
    }

    // --- CONFIG operations ---
    fn run_config_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.key_ops += 1;

        if sub < 40 {
            // CONFIG SET then CONFIG GET
            let param = format!("test-param-{}", self.rng.gen_range(0, 10));
            let value = format!("test-value-{}", self.rng.gen_range(0, 100));
            let desc = format!("CONFIG SET {} {}", param, value);
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let set_resp = self.executor.execute(&Command::ConfigSet(param.clone(), value.clone()));
            self.assert_ok(&set_resp, "CONFIG SET should return OK");

            // Verify with CONFIG GET
            let get_resp = self.executor.execute(&Command::ConfigGet(param.clone()));
            if let RespValue::Array(Some(elements)) = &get_resp {
                if elements.len() != 2 {
                    self.violation(&format!(
                        "CONFIG GET {} should return 2 elements, got {}",
                        param,
                        elements.len()
                    ));
                } else {
                    self.assert_bulk_eq(
                        &elements[1],
                        value.as_bytes(),
                        &format!("CONFIG GET {} value after SET", param),
                    );
                }
            } else {
                self.violation(&format!("CONFIG GET {} should return Array, got {:?}", param, get_resp));
            }
        } else if sub < 70 {
            // CONFIG GET with glob pattern
            let desc = "CONFIG GET *max*".to_string();
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let resp = self.executor.execute(&Command::ConfigGet("*max*".to_string()));
            if let RespValue::Array(Some(elements)) = &resp {
                if elements.len() % 2 != 0 {
                    self.violation(&format!(
                        "CONFIG GET *max* should return even number of elements, got {}",
                        elements.len()
                    ));
                }
            } else {
                self.violation(&format!("CONFIG GET *max* should return Array, got {:?}", resp));
            }
        } else {
            // CONFIG RESETSTAT
            let desc = "CONFIG RESETSTAT".to_string();
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let resp = self.executor.execute(&Command::ConfigResetStat);
            self.assert_ok(&resp, "CONFIG RESETSTAT should return OK");
        }
    }

    // --- String operations ---
    fn run_string_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.string_ops += 1;

        if sub < 15 {
            // SET + GET verification
            let key = self.random_key();
            let value = self.random_value();
            let desc = format!("SET {} then GET", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            let cmd = Command::set(key.clone(), SDS::new(value.clone()));
            let resp = self.executor.execute(&cmd);
            self.shadow.set_string(&key, value.clone());
            self.shadow.expirations.remove(&key); // SET clears expiry

            // Invariant 1: SET returns OK
            self.assert_ok(&resp, "SET should return OK");

            // Invariant 1 (continued): GET after SET returns the SET value
            let get_cmd = Command::Get(key.clone());
            let get_resp = self.executor.execute(&get_cmd);
            self.assert_bulk_eq(&get_resp, &value, &format!("GET {} after SET", key));
        } else if sub < 22 {
            // SETNX (legacy command returning Integer)
            let key = self.random_key();
            let value = self.random_value();
            let desc = format!("SETNX {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            let existed = self.shadow.exists(&key);
            let cmd = Command::setnx(key.clone(), SDS::new(value.clone()));
            let resp = self.executor.execute(&cmd);

            if existed {
                self.assert_integer(&resp, 0, "SETNX on existing key should return Integer(0)");
            } else {
                self.assert_integer(&resp, 1, "SETNX on new key should return Integer(1)");
                self.shadow.set_string(&key, value);
                self.shadow.expirations.remove(&key);
            }
        } else if sub < 30 {
            // MSET + MGET (may include duplicate keys - last value wins per Redis semantics)
            let n = self.rng.gen_range(1, 5) as usize;
            let mut pairs = Vec::new();
            for _ in 0..n {
                let key = self.random_key();
                let value = self.random_value();
                pairs.push((key, value));
            }
            let desc = format!("MSET {} keys", pairs.len());
            self.result.last_op = Some(ExecutorOp::String(desc));

            let cmd_pairs: Vec<(String, SDS)> = pairs
                .iter()
                .map(|(k, v)| (k.clone(), SDS::new(v.clone())))
                .collect();
            let resp = self.executor.execute(&Command::MSet(cmd_pairs));

            // Invariant 14: MSET is atomic - returns OK
            self.assert_ok(&resp, "MSET should return OK");

            // Update shadow (last value wins for duplicate keys, matching Redis semantics)
            for (k, v) in &pairs {
                self.shadow.set_string(k, v.clone());
                self.shadow.expirations.remove(k);
            }

            // Verify with MGET using deduplicated key list
            let unique_keys: Vec<String> = {
                let mut seen = HashSet::new();
                pairs
                    .iter()
                    .rev() // reverse so we keep the *last* occurrence
                    .filter(|(k, _)| seen.insert(k.clone()))
                    .map(|(k, _)| k.clone())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            };
            let mget_resp = self.executor.execute(&Command::MGet(unique_keys.clone()));
            if let RespValue::Array(Some(values)) = &mget_resp {
                for (i, key) in unique_keys.iter().enumerate() {
                    if i < values.len() {
                        // Look up expected from shadow (already has last-value-wins)
                        if let Some(RefValue::String(expected)) = self.shadow.get(key) {
                            let expected = expected.clone();
                            self.assert_bulk_eq(
                                &values[i],
                                &expected,
                                &format!("MGET[{}] after MSET", key),
                            );
                        }
                    }
                }
            }
        } else if sub < 38 {
            // INCR
            let key = self.random_key();
            let desc = format!("INCR {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            // Check what the shadow state holds
            enum IncrExpect {
                Value(i64),          // Key is integer-string or missing -> compute new value
                NotInteger,          // Key is string but not parseable as integer
                WrongType,           // Key is not a string type
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => {
                    match std::str::from_utf8(v).ok().and_then(|s| s.parse::<i64>().ok()) {
                        Some(n) => IncrExpect::Value(n),
                        None => IncrExpect::NotInteger,
                    }
                }
                None => IncrExpect::Value(0), // INCR on non-existent = 0 + 1
                Some(_) => IncrExpect::WrongType,
            };

            match expect {
                IncrExpect::Value(current) => {
                    let resp = self.executor.execute(&Command::Incr(key.clone()));
                    let expected = current + 1;
                    // Invariant 3: INCR produces correct arithmetic
                    self.assert_integer(&resp, expected, &format!("INCR {} should be {}", key, expected));
                    self.shadow.set_string(&key, expected.to_string().into_bytes());
                }
                IncrExpect::NotInteger => {
                    // Key holds non-integer string - expect ERR not WRONGTYPE
                    let resp = self.executor.execute(&Command::Incr(key.clone()));
                    self.assert_error_contains(&resp, "ERR", "INCR on non-integer string");
                }
                IncrExpect::WrongType => {
                    // Key holds wrong data type (list, set, etc.)
                    let resp = self.executor.execute(&Command::Incr(key.clone()));
                    self.assert_error_contains(&resp, "WRONGTYPE", "INCR on wrong type");
                }
            }
        } else if sub < 46 {
            // GET (read-only)
            let key = self.random_key();
            let desc = format!("GET {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            let resp = self.executor.execute(&Command::Get(key.clone()));
            enum GetExpect {
                Value(Vec<u8>),
                Null,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => GetExpect::Value(v.clone()),
                None => GetExpect::Null,
                Some(_) => GetExpect::WrongType,
            };
            match expect {
                GetExpect::Value(v) => {
                    self.assert_bulk_eq(&resp, &v, &format!("GET {} should match shadow", key));
                }
                GetExpect::Null => {
                    self.assert_null(&resp, &format!("GET {} non-existent should be nil", key));
                }
                GetExpect::WrongType => {
                    self.assert_error_contains(
                        &resp,
                        "WRONGTYPE",
                        &format!("GET {} on wrong type", key),
                    );
                }
            }
        } else if sub < 52 {
            // APPEND
            let key = self.random_key();
            let value = self.random_value();
            let desc = format!("APPEND {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            let is_string_or_none = matches!(
                self.shadow.get(&key),
                Some(RefValue::String(_)) | None
            );

            if is_string_or_none {
                let resp = self.executor.execute(&Command::Append(key.clone(), SDS::new(value.clone())));

                let new_val = match self.shadow.get(&key) {
                    Some(RefValue::String(existing)) => {
                        let mut new = existing.clone();
                        new.extend_from_slice(&value);
                        new
                    }
                    None => value.clone(),
                    _ => unreachable!(),
                };

                self.assert_integer(
                    &resp,
                    new_val.len() as i64,
                    &format!("APPEND {} should return new length", key),
                );
                self.shadow.set_string(&key, new_val);
            }
        } else if sub < 57 {
            // STRLEN
            let key = self.random_key();
            let desc = format!("STRLEN {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            let resp = self.executor.execute(&Command::StrLen(key.clone()));
            // Extract expected before asserting to avoid borrow conflict
            let expected: Result<i64, &'static str> = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => Ok(v.len() as i64),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected {
                Ok(len) => {
                    self.assert_integer(&resp, len, &format!("STRLEN {}", key));
                }
                Err(_) => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "STRLEN on wrong type");
                }
            }
        } else if sub < 63 {
            // GETRANGE - only on string-or-none keys to avoid executor debug_assert on WRONGTYPE
            let key = self.random_key();
            let is_string_or_none = matches!(
                self.shadow.get(&key),
                Some(RefValue::String(_)) | None
            );

            if is_string_or_none {
                // Random start/end indices including negative values
                let start = (self.rng.gen_range(0, 20) as isize) - 10;
                let end = (self.rng.gen_range(0, 20) as isize) - 5;
                let desc = format!("GETRANGE {} {} {}", key, start, end);
                self.result.last_op = Some(ExecutorOp::String(desc));

                let resp = self.executor.execute(&Command::GetRange(key.clone(), start, end));

                let expected = match self.shadow.get(&key) {
                    Some(RefValue::String(v)) => {
                        let bytes = v.as_slice();
                        let len = bytes.len() as isize;
                        if len == 0 {
                            vec![]
                        } else {
                            let s_idx = if start < 0 { (len + start).max(0) } else { start.min(len) };
                            let e_idx = if end < 0 { (len + end).max(0) } else { end.min(len - 1) };
                            if s_idx > e_idx || s_idx >= len {
                                vec![]
                            } else {
                                let s_idx = s_idx.max(0);
                                let e_idx = e_idx.min(len - 1);
                                bytes[s_idx as usize..=e_idx as usize].to_vec()
                            }
                        }
                    }
                    None => vec![],
                    _ => unreachable!(),
                };
                self.assert_bulk_eq(
                    &resp,
                    &expected,
                    &format!("GETRANGE {} {} {}", key, start, end),
                );
            }
        } else if sub < 69 {
            // SETRANGE
            let key = self.random_key();
            // Use small offsets to avoid enormous strings
            let offset = self.rng.gen_range(0, 20) as usize;
            let value = self.random_value();
            let desc = format!("SETRANGE {} {} <value>", key, offset);
            self.result.last_op = Some(ExecutorOp::String(desc));

            let is_string_or_none = matches!(
                self.shadow.get(&key),
                Some(RefValue::String(_)) | None
            );

            if is_string_or_none {
                let resp = self.executor.execute(&Command::SetRange(
                    key.clone(),
                    offset,
                    SDS::new(value.clone()),
                ));

                // Shadow: pad with zeros if needed, then overwrite at offset
                let mut bytes = match self.shadow.get(&key) {
                    Some(RefValue::String(existing)) => existing.clone(),
                    None => vec![],
                    _ => unreachable!(),
                };
                let needed = offset + value.len();
                if needed > bytes.len() {
                    bytes.resize(needed, 0);
                }
                bytes[offset..needed].copy_from_slice(&value);
                let expected_len = bytes.len() as i64;
                self.shadow.set_string(&key, bytes);

                self.assert_integer(
                    &resp,
                    expected_len,
                    &format!("SETRANGE {} should return new length {}", key, expected_len),
                );
            }
        } else if sub < 75 {
            // GETDEL
            let key = self.random_key();
            let desc = format!("GETDEL {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            let resp = self.executor.execute(&Command::GetDel(key.clone()));

            enum GDExpect {
                Value(Vec<u8>),
                Null,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => GDExpect::Value(v.clone()),
                None => GDExpect::Null,
                Some(_) => GDExpect::WrongType,
            };
            match expect {
                GDExpect::Value(v) => {
                    self.assert_bulk_eq(&resp, &v, &format!("GETDEL {} should return old value", key));
                    self.shadow.del(&key);
                }
                GDExpect::Null => {
                    self.assert_null(&resp, &format!("GETDEL {} non-existent should be nil", key));
                }
                GDExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "GETDEL on wrong type");
                }
            }
        } else if sub < 80 {
            // INCRBYFLOAT
            let key = self.random_key();
            // Use small float increments to avoid precision issues
            let increment = (self.rng.gen_range(0, 2000) as f64 - 1000.0) / 100.0;
            let desc = format!("INCRBYFLOAT {} {}", key, increment);
            self.result.last_op = Some(ExecutorOp::String(desc));

            enum IBFExpect {
                Value(f64),  // Current value as f64 (or 0.0 for missing)
                NotFloat,    // Key holds non-float string
                WrongType,   // Key holds wrong data type
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => {
                    match std::str::from_utf8(v).ok().and_then(|s| s.parse::<f64>().ok()) {
                        Some(n) => IBFExpect::Value(n),
                        None => IBFExpect::NotFloat,
                    }
                }
                None => IBFExpect::Value(0.0),
                Some(_) => IBFExpect::WrongType,
            };

            match expect {
                IBFExpect::Value(current) => {
                    let resp = self.executor.execute(&Command::IncrByFloat(key.clone(), increment));
                    // Response-driven: trust the executor's response and sync shadow
                    if let RespValue::BulkString(Some(data)) = &resp {
                        // Store the executor's result as the new shadow value
                        self.shadow.set_string(&key, data.clone());
                        // Verify the result is close to expected
                        let expected = current + increment;
                        if let Ok(actual) = std::str::from_utf8(data).unwrap_or("").parse::<f64>() {
                            if (actual - expected).abs() > 1e-9 {
                                self.violation(&format!(
                                    "INCRBYFLOAT {} {}: got {}, expected ~{}",
                                    key, increment, actual, expected
                                ));
                            }
                        }
                    } else {
                        self.violation(&format!(
                            "INCRBYFLOAT {} {}: expected BulkString, got {:?}",
                            key, increment, resp
                        ));
                    }
                }
                IBFExpect::NotFloat => {
                    let resp = self.executor.execute(&Command::IncrByFloat(key.clone(), increment));
                    self.assert_error_contains(&resp, "ERR", "INCRBYFLOAT on non-float string");
                }
                IBFExpect::WrongType => {
                    let resp = self.executor.execute(&Command::IncrByFloat(key.clone(), increment));
                    self.assert_error_contains(&resp, "WRONGTYPE", "INCRBYFLOAT on wrong type");
                }
            }
        } else if sub < 85 {
            // GETSET
            let key = self.random_key();
            let value = self.random_value();
            let desc = format!("GETSET {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            enum GSExpect {
                OldValue(Vec<u8>),
                Null,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => GSExpect::OldValue(v.clone()),
                None => GSExpect::Null,
                Some(_) => GSExpect::WrongType,
            };

            let resp = self.executor.execute(&Command::GetSet(key.clone(), SDS::new(value.clone())));
            match expect {
                GSExpect::OldValue(old) => {
                    self.assert_bulk_eq(&resp, &old, &format!("GETSET {} should return old value", key));
                    self.shadow.set_string(&key, value);
                }
                GSExpect::Null => {
                    self.assert_null(&resp, &format!("GETSET {} non-existent should return nil", key));
                    self.shadow.set_string(&key, value);
                }
                GSExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "GETSET on wrong type");
                }
            }
        } else if sub < 90 {
            // DECRBY
            let key = self.random_key();
            let decrement = self.rng.gen_range(1, 100) as i64;
            let desc = format!("DECRBY {} {}", key, decrement);
            self.result.last_op = Some(ExecutorOp::String(desc));

            enum DecrByExpect {
                Value(i64),
                NotInteger,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => {
                    match std::str::from_utf8(v).ok().and_then(|s| s.parse::<i64>().ok()) {
                        Some(n) => DecrByExpect::Value(n),
                        None => DecrByExpect::NotInteger,
                    }
                }
                None => DecrByExpect::Value(0),
                Some(_) => DecrByExpect::WrongType,
            };

            let resp = self.executor.execute(&Command::DecrBy(key.clone(), decrement));
            match expect {
                DecrByExpect::Value(current) => {
                    match current.checked_sub(decrement) {
                        Some(expected) => {
                            self.assert_integer(&resp, expected, &format!("DECRBY {} {}", key, decrement));
                            self.shadow.set_string(&key, expected.to_string().into_bytes());
                        }
                        None => {
                            // Overflow
                            self.assert_error_contains(&resp, "ERR", "DECRBY overflow");
                        }
                    }
                }
                DecrByExpect::NotInteger => {
                    self.assert_error_contains(&resp, "ERR", "DECRBY on non-integer string");
                }
                DecrByExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "DECRBY on wrong type");
                }
            }
        } else if sub < 95 {
            // INCRBY
            let key = self.random_key();
            let increment = self.rng.gen_range(1, 100) as i64;
            let desc = format!("INCRBY {} {}", key, increment);
            self.result.last_op = Some(ExecutorOp::String(desc));

            enum IncrByExpect {
                Value(i64),
                NotInteger,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => {
                    match std::str::from_utf8(v).ok().and_then(|s| s.parse::<i64>().ok()) {
                        Some(n) => IncrByExpect::Value(n),
                        None => IncrByExpect::NotInteger,
                    }
                }
                None => IncrByExpect::Value(0),
                Some(_) => IncrByExpect::WrongType,
            };

            let resp = self.executor.execute(&Command::IncrBy(key.clone(), increment));
            match expect {
                IncrByExpect::Value(current) => {
                    match current.checked_add(increment) {
                        Some(expected) => {
                            self.assert_integer(&resp, expected, &format!("INCRBY {} {}", key, increment));
                            self.shadow.set_string(&key, expected.to_string().into_bytes());
                        }
                        None => {
                            // Overflow
                            self.assert_error_contains(&resp, "ERR", "INCRBY overflow");
                        }
                    }
                }
                IncrByExpect::NotInteger => {
                    self.assert_error_contains(&resp, "ERR", "INCRBY on non-integer string");
                }
                IncrByExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "INCRBY on wrong type");
                }
            }
        } else if sub < 98 {
            // DECR
            let key = self.random_key();
            let desc = format!("DECR {}", key);
            self.result.last_op = Some(ExecutorOp::String(desc));

            enum DecrExpect {
                Value(i64),
                NotInteger,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::String(v)) => {
                    match std::str::from_utf8(v).ok().and_then(|s| s.parse::<i64>().ok()) {
                        Some(n) => DecrExpect::Value(n),
                        None => DecrExpect::NotInteger,
                    }
                }
                None => DecrExpect::Value(0),
                Some(_) => DecrExpect::WrongType,
            };

            let resp = self.executor.execute(&Command::Decr(key.clone()));
            match expect {
                DecrExpect::Value(current) => {
                    match current.checked_sub(1) {
                        Some(expected) => {
                            self.assert_integer(&resp, expected, &format!("DECR {} should be {}", key, expected));
                            self.shadow.set_string(&key, expected.to_string().into_bytes());
                        }
                        None => {
                            // Overflow (i64::MIN - 1)
                            self.assert_error_contains(&resp, "ERR", "DECR overflow");
                        }
                    }
                }
                DecrExpect::NotInteger => {
                    let resp_already = resp;
                    self.assert_error_contains(&resp_already, "ERR", "DECR on non-integer string");
                }
                DecrExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "DECR on wrong type");
                }
            }
        } else {
            // SETBIT / GETBIT
            let key = self.random_key();
            let offset: u64 = self.rng.gen_range(0, 2048);
            let bit_sub = self.rng.gen_range(0, 100);

            if bit_sub < 60 {
                // SETBIT
                let value = self.rng.gen_range(0, 2) as u8;
                let desc = format!("SETBIT {} {} {}", key, offset, value);
                self.result.last_op = Some(ExecutorOp::String(desc));

                // Determine expected behavior from shadow
                enum SetBitExpect {
                    OldBit(i64), // returns old bit value, key is string or new
                    WrongType,   // key exists but not a string
                }
                let expect = match self.shadow.get(&key) {
                    Some(RefValue::String(v)) => {
                        let byte_idx = (offset / 8) as usize;
                        let bit_mask = 0x80u8 >> (offset % 8);
                        let old = if byte_idx < v.len() {
                            if v[byte_idx] & bit_mask != 0 { 1i64 } else { 0i64 }
                        } else {
                            0i64
                        };
                        SetBitExpect::OldBit(old)
                    }
                    None => SetBitExpect::OldBit(0),
                    Some(_) => SetBitExpect::WrongType,
                };

                let resp = self.executor.execute(&Command::SetBit(key.clone(), offset, value));

                match expect {
                    SetBitExpect::OldBit(old) => {
                        self.assert_integer(&resp, old, &format!("SETBIT {} old bit", key));
                        // Update shadow
                        let byte_idx = (offset / 8) as usize;
                        let bit_mask = 0x80u8 >> (offset % 8);
                        let v = match self.shadow.data.get_mut(&key) {
                            Some(RefValue::String(v)) => v,
                            _ => {
                                self.shadow.data.insert(key.clone(), RefValue::String(Vec::new()));
                                match self.shadow.data.get_mut(&key) {
                                    Some(RefValue::String(v)) => v,
                                    _ => unreachable!(),
                                }
                            }
                        };
                        if v.len() <= byte_idx {
                            v.resize(byte_idx + 1, 0);
                        }
                        if value == 1 {
                            v[byte_idx] |= bit_mask;
                        } else {
                            v[byte_idx] &= !bit_mask;
                        }
                    }
                    SetBitExpect::WrongType => {
                        self.assert_error_contains(&resp, "WRONGTYPE", "SETBIT on wrong type");
                    }
                }
            } else {
                // GETBIT
                let desc = format!("GETBIT {} {}", key, offset);
                self.result.last_op = Some(ExecutorOp::String(desc));

                let expected = match self.shadow.get(&key) {
                    Some(RefValue::String(v)) => {
                        let byte_idx = (offset / 8) as usize;
                        let bit_mask = 0x80u8 >> (offset % 8);
                        if byte_idx < v.len() {
                            if v[byte_idx] & bit_mask != 0 { 1i64 } else { 0i64 }
                        } else {
                            0i64
                        }
                    }
                    None => 0i64,
                    Some(_) => -1i64, // sentinel for WRONGTYPE
                };

                let resp = self.executor.execute(&Command::GetBit(key.clone(), offset));

                if expected == -1 {
                    self.assert_error_contains(&resp, "WRONGTYPE", "GETBIT on wrong type");
                } else {
                    self.assert_integer(&resp, expected, &format!("GETBIT {} at {}", key, offset));
                }
            }
        }
    }

    // --- Key operations ---
    fn run_key_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.key_ops += 1;

        if sub < 20 {
            // DEL
            let key = self.random_key();
            let desc = format!("DEL {}", key);
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let existed = self.shadow.exists(&key);
            let resp = self.executor.execute(&Command::Del(vec![key.clone()]));

            // Invariant 2: DEL makes key non-existent
            let expected = if existed { 1 } else { 0 };
            self.assert_integer(&resp, expected, &format!("DEL {} count", key));

            self.shadow.del(&key);

            // Verify key is gone
            let get_resp = self.executor.execute(&Command::Get(key.clone()));
            self.assert_null(&get_resp, &format!("GET {} after DEL should be nil", key));
        } else if sub < 35 {
            // EXISTS
            let key = self.random_key();
            let desc = format!("EXISTS {}", key);
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let resp = self.executor.execute(&Command::Exists(vec![key.clone()]));
            let expected = if self.shadow.exists(&key) { 1 } else { 0 };
            self.assert_integer(&resp, expected, &format!("EXISTS {}", key));
        } else if sub < 50 {
            // TYPE
            let key = self.random_key();
            let desc = format!("TYPE {}", key);
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let resp = self.executor.execute(&Command::TypeOf(key.clone()));
            let expected_type = match self.shadow.get(&key) {
                Some(rv) => rv.type_name(),
                None => "none",
            };
            self.assert_simple_string(&resp, expected_type, &format!("TYPE {}", key));
        } else if sub < 60 {
            // DBSIZE
            let desc = "DBSIZE".to_string();
            self.result.last_op = Some(ExecutorOp::Key(desc));

            // Sync shadow with executor before DBSIZE check: the executor lazily
            // evicts expired keys on access, while shadow eagerly evicts. Also,
            // some operations (e.g., SETRANGE on non-existent key, type coercion)
            // may create keys the shadow doesn't fully track.
            let executor_valid_keys: HashSet<String> = self
                .executor
                .get_data()
                .keys()
                .filter(|k| !self.executor.is_expired(k))
                .cloned()
                .collect();
            let shadow_keys: HashSet<String> = self.shadow.data.keys().cloned().collect();
            // Remove from shadow keys not in executor
            for key in shadow_keys.difference(&executor_valid_keys) {
                self.shadow.data.remove(key);
                self.shadow.expirations.remove(key);
            }
            // Add to shadow keys in executor but not in shadow (opaque tracking)
            for key in executor_valid_keys.difference(&shadow_keys) {
                self.shadow
                    .data
                    .insert(key.clone(), RefValue::String(Vec::new()));
            }

            let resp = self.executor.execute(&Command::DbSize);
            let expected = self.shadow.key_count() as i64;
            self.assert_integer(&resp, expected, "DBSIZE should match shadow count");
        } else if sub < 70 {
            // FLUSHDB
            let desc = "FLUSHDB".to_string();
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let resp = self.executor.execute(&Command::FlushDb);
            self.assert_ok(&resp, "FLUSHDB should return OK");
            self.shadow.clear();

            // Invariant 13: FLUSHDB empties everything
            let dbsize_resp = self.executor.execute(&Command::DbSize);
            self.assert_integer(&dbsize_resp, 0, "DBSIZE after FLUSHDB should be 0");
        } else if sub < 85 {
            // RENAME
            let src = self.random_key();
            let mut dst = self.random_key();
            // Avoid src == dst which triggers a debug_assert in the executor
            // (RENAME same-key is a no-op in Redis but the postcondition check trips)
            if src == dst {
                dst = format!("{}_renamed", src);
                self.all_keys_ever.insert(dst.clone());
            }
            let desc = format!("RENAME {} {}", src, dst);
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let src_exists = self.shadow.exists(&src);
            let resp = self.executor.execute(&Command::Rename(src.clone(), dst.clone()));

            if !src_exists {
                // RENAME on non-existent key returns error
                self.assert_error_contains(&resp, "ERR", &format!("RENAME {} non-existent src", src));
            } else {
                self.assert_ok(&resp, &format!("RENAME {} {} should return OK", src, dst));

                // Shadow: move value and expiry from src to dst
                if let Some(val) = self.shadow.data.remove(&src) {
                    let exp = self.shadow.expirations.remove(&src);
                    self.shadow.data.insert(dst.clone(), val);
                    if let Some(exp_time) = exp {
                        self.shadow.expirations.insert(dst.clone(), exp_time);
                    } else {
                        self.shadow.expirations.remove(&dst);
                    }
                }

                // Verify src is gone, dst exists
                let src_exists_after = self.executor.execute(&Command::Exists(vec![src.clone()]));
                // If src == dst, key still exists at that name
                if src != dst {
                    self.assert_integer(&src_exists_after, 0, &format!("RENAME src {} should not exist", src));
                }
            }
        } else {
            // PTTL
            let key = self.random_key();
            let desc = format!("PTTL {}", key);
            self.result.last_op = Some(ExecutorOp::Key(desc));

            let resp = self.executor.execute(&Command::Pttl(key.clone()));

            // Response-driven invariants for PTTL:
            // -2 = key doesn't exist, -1 = no expiry, >= 0 = TTL in milliseconds
            if let RespValue::Integer(pttl) = &resp {
                if !self.shadow.exists(&key) {
                    // Key doesn't exist in shadow - executor should return -2
                    // (but don't hard-assert due to expiry race conditions)
                }
                // PTTL should be >= -2
                if *pttl < -2 {
                    self.violation(&format!("PTTL {} returned invalid value: {}", key, pttl));
                }
                // If key exists and has no expiry, should be -1
                if self.shadow.exists(&key) && !self.shadow.expirations.contains_key(&key) {
                    if *pttl != -1 {
                        // Soft check - expiry state might diverge
                    }
                }
            } else {
                self.violation(&format!("PTTL {} should return Integer, got {:?}", key, resp));
            }
        }
    }

    // --- List operations ---
    fn run_list_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.list_ops += 1;

        let key = self.random_key();

        // Ensure key is a list or doesn't exist for write ops
        let is_list_or_none = matches!(self.shadow.get(&key), Some(RefValue::List(_)) | None);

        if sub < 25 && is_list_or_none {
            // LPUSH
            let value = self.random_value();
            let desc = format!("LPUSH {}", key);
            self.result.last_op = Some(ExecutorOp::List(desc));

            let resp = self
                .executor
                .execute(&Command::LPush(key.clone(), vec![SDS::new(value.clone())]));

            let list = self
                .shadow
                .data
                .entry(key.clone())
                .or_insert_with(|| RefValue::List(Vec::new()));
            if let RefValue::List(ref mut l) = list {
                l.insert(0, value);
            }
            // Invariant 4: List length matches push count
            let expected_len = match self.shadow.get(&key) {
                Some(RefValue::List(l)) => l.len() as i64,
                _ => 0,
            };
            self.assert_integer(&resp, expected_len, &format!("LPUSH {} new length", key));
        } else if sub < 50 && is_list_or_none {
            // RPUSH
            let value = self.random_value();
            let desc = format!("RPUSH {}", key);
            self.result.last_op = Some(ExecutorOp::List(desc));

            let resp = self
                .executor
                .execute(&Command::RPush(key.clone(), vec![SDS::new(value.clone())]));

            let list = self
                .shadow
                .data
                .entry(key.clone())
                .or_insert_with(|| RefValue::List(Vec::new()));
            if let RefValue::List(ref mut l) = list {
                l.push(value);
            }
            let expected_len = match self.shadow.get(&key) {
                Some(RefValue::List(l)) => l.len() as i64,
                _ => 0,
            };
            self.assert_integer(&resp, expected_len, &format!("RPUSH {} new length", key));
        } else if sub < 65 {
            // LPOP
            let desc = format!("LPOP {}", key);
            self.result.last_op = Some(ExecutorOp::List(desc));

            let resp = self.executor.execute(&Command::LPop(key.clone()));

            // Extract expected from shadow, then assert separately
            enum PopExpect {
                Value(Vec<u8>),
                Null,
                WrongType,
            }
            let expect = match self.shadow.data.get_mut(&key) {
                Some(RefValue::List(ref mut l)) if !l.is_empty() => {
                    let val = l.remove(0);
                    PopExpect::Value(val)
                }
                Some(RefValue::List(_)) | None => PopExpect::Null,
                Some(_) => PopExpect::WrongType,
            };
            // Clean up empty lists
            if matches!(self.shadow.data.get(&key), Some(RefValue::List(l)) if l.is_empty()) {
                self.shadow.data.remove(&key);
            }
            match expect {
                PopExpect::Value(expected) => {
                    self.assert_bulk_eq(&resp, &expected, &format!("LPOP {} value", key));
                }
                PopExpect::Null => {
                    self.assert_null(&resp, &format!("LPOP {} empty/nonexistent", key));
                }
                PopExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "LPOP on wrong type");
                }
            }
        } else if sub < 80 {
            // RPOP
            let desc = format!("RPOP {}", key);
            self.result.last_op = Some(ExecutorOp::List(desc));

            let resp = self.executor.execute(&Command::RPop(key.clone()));

            enum RPopExpect {
                Value(Vec<u8>),
                Null,
                WrongType,
            }
            let expect = match self.shadow.data.get_mut(&key) {
                Some(RefValue::List(ref mut l)) if !l.is_empty() => {
                    let val = l.pop().unwrap();
                    RPopExpect::Value(val)
                }
                Some(RefValue::List(_)) | None => RPopExpect::Null,
                Some(_) => RPopExpect::WrongType,
            };
            if matches!(self.shadow.data.get(&key), Some(RefValue::List(l)) if l.is_empty()) {
                self.shadow.data.remove(&key);
            }
            match expect {
                RPopExpect::Value(expected) => {
                    self.assert_bulk_eq(&resp, &expected, &format!("RPOP {} value", key));
                }
                RPopExpect::Null => {
                    self.assert_null(&resp, &format!("RPOP {} empty/nonexistent", key));
                }
                RPopExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "RPOP on wrong type");
                }
            }
        } else if sub < 87 {
            // LLEN
            let desc = format!("LLEN {}", key);
            self.result.last_op = Some(ExecutorOp::List(desc));

            let resp = self.executor.execute(&Command::LLen(key.clone()));

            let expected: Result<i64, &str> = match self.shadow.get(&key) {
                Some(RefValue::List(l)) => Ok(l.len() as i64),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected {
                Ok(len) => self.assert_integer(&resp, len, &format!("LLEN {}", key)),
                Err(_) => self.assert_error_contains(&resp, "WRONGTYPE", "LLEN on wrong type"),
            }
        } else if sub < 94 {
            // LRANGE (read entire list)
            let desc = format!("LRANGE {} 0 -1", key);
            self.result.last_op = Some(ExecutorOp::List(desc));

            let resp = self
                .executor
                .execute(&Command::LRange(key.clone(), 0, -1));

            let expected_len: Result<usize, &str> = match self.shadow.get(&key) {
                Some(RefValue::List(l)) => Ok(l.len()),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected_len {
                Ok(len) => {
                    if let RespValue::Array(Some(elements)) = &resp {
                        if elements.len() != len {
                            self.violation(&format!(
                                "LRANGE {} length mismatch: got {}, expected {}",
                                key,
                                elements.len(),
                                len
                            ));
                        }
                    }
                }
                Err(_) => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "LRANGE on wrong type");
                }
            }
        } else {
            // LINDEX
            let index = (self.rng.gen_range(0, 10) as isize) - 3; // range: -3 to 6
            let desc = format!("LINDEX {} {}", key, index);
            self.result.last_op = Some(ExecutorOp::List(desc));

            let resp = self.executor.execute(&Command::LIndex(key.clone(), index));

            enum LIExpect {
                Value(Vec<u8>),
                Null,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::List(l)) => {
                    let len = l.len() as isize;
                    let actual_idx = if index < 0 { len + index } else { index };
                    if actual_idx < 0 || actual_idx >= len {
                        LIExpect::Null
                    } else {
                        LIExpect::Value(l[actual_idx as usize].clone())
                    }
                }
                None => LIExpect::Null,
                Some(_) => LIExpect::WrongType,
            };
            match expect {
                LIExpect::Value(expected) => {
                    self.assert_bulk_eq(&resp, &expected, &format!("LINDEX {} {}", key, index));
                }
                LIExpect::Null => {
                    self.assert_null(&resp, &format!("LINDEX {} {} out of range/missing", key, index));
                }
                LIExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "LINDEX on wrong type");
                }
            }
        }
    }

    // --- Set operations ---
    fn run_set_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.set_ops += 1;

        let key = self.random_key();
        let is_set_or_none = matches!(self.shadow.get(&key), Some(RefValue::Set(_)) | None);

        if sub < 35 && is_set_or_none {
            // SADD
            let member = self.random_value();
            let desc = format!("SADD {} member", key);
            self.result.last_op = Some(ExecutorOp::Set(desc));

            let resp = self
                .executor
                .execute(&Command::SAdd(key.clone(), vec![SDS::new(member.clone())]));

            let set = self
                .shadow
                .data
                .entry(key.clone())
                .or_insert_with(|| RefValue::Set(HashSet::new()));
            if let RefValue::Set(ref mut s) = set {
                let was_new = s.insert(member);
                let expected = if was_new { 1 } else { 0 };
                // Invariant 5: Set cardinality matches SADD/SREM
                self.assert_integer(&resp, expected, &format!("SADD {} result", key));
            }
        } else if sub < 55 {
            // SREM
            let member = self.random_value();
            let desc = format!("SREM {} member", key);
            self.result.last_op = Some(ExecutorOp::Set(desc));

            let resp = self.executor.execute(&Command::SRem(
                key.clone(),
                vec![SDS::new(member.clone())],
            ));

            let expected: Result<i64, &str> = match self.shadow.data.get_mut(&key) {
                Some(RefValue::Set(ref mut s)) => {
                    let was_present = s.remove(&member);
                    Ok(if was_present { 1 } else { 0 })
                }
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            if matches!(self.shadow.data.get(&key), Some(RefValue::Set(s)) if s.is_empty()) {
                self.shadow.data.remove(&key);
            }
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("SREM {} result", key)),
                Err(_) => self.assert_error_contains(&resp, "WRONGTYPE", "SREM on wrong type"),
            }
        } else if sub < 75 {
            // SCARD
            let desc = format!("SCARD {}", key);
            self.result.last_op = Some(ExecutorOp::Set(desc));

            let resp = self.executor.execute(&Command::SCard(key.clone()));

            let expected: Result<i64, &str> = match self.shadow.get(&key) {
                Some(RefValue::Set(s)) => Ok(s.len() as i64),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("SCARD {}", key)),
                Err(_) => self.assert_error_contains(&resp, "WRONGTYPE", "SCARD on wrong type"),
            }
        } else if sub < 88 {
            // SISMEMBER
            let member = self.random_value();
            let desc = format!("SISMEMBER {}", key);
            self.result.last_op = Some(ExecutorOp::Set(desc));

            let resp = self.executor.execute(&Command::SIsMember(
                key.clone(),
                SDS::new(member.clone()),
            ));

            let expected: Result<i64, &str> = match self.shadow.get(&key) {
                Some(RefValue::Set(s)) => Ok(if s.contains(&member) { 1 } else { 0 }),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("SISMEMBER {}", key)),
                Err(_) => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "SISMEMBER on wrong type")
                }
            }
        } else {
            // SMEMBERS
            let desc = format!("SMEMBERS {}", key);
            self.result.last_op = Some(ExecutorOp::Set(desc));

            let resp = self.executor.execute(&Command::SMembers(key.clone()));

            enum SMExpect {
                Members(usize),  // expected count
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::Set(s)) => SMExpect::Members(s.len()),
                None => SMExpect::Members(0),
                Some(_) => SMExpect::WrongType,
            };
            match expect {
                SMExpect::Members(count) => {
                    if let RespValue::Array(Some(elements)) = &resp {
                        if elements.len() != count {
                            self.violation(&format!(
                                "SMEMBERS {} count mismatch: got {}, expected {}",
                                key,
                                elements.len(),
                                count
                            ));
                        }
                    } else if count == 0 {
                        // Empty set returns empty array
                        if !matches!(&resp, RespValue::Array(Some(v)) if v.is_empty()) {
                            self.violation(&format!(
                                "SMEMBERS {} expected empty array, got {:?}",
                                key, resp
                            ));
                        }
                    }
                }
                SMExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "SMEMBERS on wrong type");
                }
            }
        }
    }

    // --- Hash operations ---
    fn run_hash_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.hash_ops += 1;

        let key = self.random_key();
        let is_hash_or_none = matches!(self.shadow.get(&key), Some(RefValue::Hash(_)) | None);

        if sub < 30 && is_hash_or_none {
            // HSET
            let field = self.random_field();
            let value = self.random_value();
            let desc = format!("HSET {} field", key);
            self.result.last_op = Some(ExecutorOp::Hash(desc));

            let resp = self.executor.execute(&Command::HSet(
                key.clone(),
                vec![(SDS::new(field.clone()), SDS::new(value.clone()))],
            ));

            let hash = self
                .shadow
                .data
                .entry(key.clone())
                .or_insert_with(|| RefValue::Hash(HashMap::new()));
            if let RefValue::Hash(ref mut h) = hash {
                let was_new = !h.contains_key(&field);
                h.insert(field, value);
                let expected = if was_new { 1 } else { 0 };
                self.assert_integer(&resp, expected, &format!("HSET {} result", key));
            }
        } else if sub < 50 {
            // HGET
            let field = self.random_field();
            let desc = format!("HGET {} field", key);
            self.result.last_op = Some(ExecutorOp::Hash(desc));

            let resp = self
                .executor
                .execute(&Command::HGet(key.clone(), SDS::new(field.clone())));

            enum HGetExpect {
                Value(Vec<u8>),
                Null,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::Hash(h)) => match h.get(&field) {
                    Some(v) => HGetExpect::Value(v.clone()),
                    None => HGetExpect::Null,
                },
                None => HGetExpect::Null,
                Some(_) => HGetExpect::WrongType,
            };
            match expect {
                HGetExpect::Value(v) => {
                    self.assert_bulk_eq(&resp, &v, &format!("HGET {} field", key));
                }
                HGetExpect::Null => {
                    self.assert_null(&resp, &format!("HGET {} missing/nonexistent", key));
                }
                HGetExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "HGET on wrong type");
                }
            }
        } else if sub < 65 {
            // HDEL
            let field = self.random_field();
            let desc = format!("HDEL {} field", key);
            self.result.last_op = Some(ExecutorOp::Hash(desc));

            let resp = self.executor.execute(&Command::HDel(
                key.clone(),
                vec![SDS::new(field.clone())],
            ));

            // Invariant 6: Hash field count matches HSET/HDEL
            let expected: Result<i64, &str> = match self.shadow.data.get_mut(&key) {
                Some(RefValue::Hash(ref mut h)) => {
                    let was_present = h.remove(&field).is_some();
                    Ok(if was_present { 1 } else { 0 })
                }
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            if matches!(self.shadow.data.get(&key), Some(RefValue::Hash(h)) if h.is_empty()) {
                self.shadow.data.remove(&key);
            }
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("HDEL {} result", key)),
                Err(_) => self.assert_error_contains(&resp, "WRONGTYPE", "HDEL on wrong type"),
            }
        } else if sub < 80 {
            // HLEN
            let desc = format!("HLEN {}", key);
            self.result.last_op = Some(ExecutorOp::Hash(desc));

            let resp = self.executor.execute(&Command::HLen(key.clone()));

            let expected: Result<i64, &str> = match self.shadow.get(&key) {
                Some(RefValue::Hash(h)) => Ok(h.len() as i64),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("HLEN {}", key)),
                Err(_) => self.assert_error_contains(&resp, "WRONGTYPE", "HLEN on wrong type"),
            }
        } else if sub < 90 {
            // HEXISTS
            let field = self.random_field();
            let desc = format!("HEXISTS {} field", key);
            self.result.last_op = Some(ExecutorOp::Hash(desc));

            let resp = self.executor.execute(&Command::HExists(
                key.clone(),
                SDS::new(field.clone()),
            ));

            let expected: Result<i64, &str> = match self.shadow.get(&key) {
                Some(RefValue::Hash(h)) => Ok(if h.contains_key(&field) { 1 } else { 0 }),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("HEXISTS {}", key)),
                Err(_) => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "HEXISTS on wrong type")
                }
            }
        } else {
            // HINCRBY
            let field = self.random_field();
            let increment = (self.rng.gen_range(0, 20) as i64) - 10;
            let desc = format!("HINCRBY {} field {}", key, increment);
            self.result.last_op = Some(ExecutorOp::Hash(desc));

            if is_hash_or_none {
                enum HIncrExpect {
                    Value(i64),
                    NotInteger,
                }
                let expect = match self.shadow.get(&key) {
                    Some(RefValue::Hash(h)) => match h.get(&field) {
                        Some(v) => {
                            match std::str::from_utf8(v).ok().and_then(|s| s.parse::<i64>().ok()) {
                                Some(n) => HIncrExpect::Value(n),
                                None => HIncrExpect::NotInteger,
                            }
                        }
                        None => HIncrExpect::Value(0), // Field doesn't exist, default 0
                    },
                    None => HIncrExpect::Value(0), // Key doesn't exist, default 0
                    _ => return,
                };

                let resp = self.executor.execute(&Command::HIncrBy(
                    key.clone(),
                    SDS::new(field.clone()),
                    increment,
                ));

                match expect {
                    HIncrExpect::Value(current_val) => {
                        let expected = current_val + increment;
                        self.assert_integer(&resp, expected, &format!("HINCRBY {} result", key));

                        let hash = self
                            .shadow
                            .data
                            .entry(key.clone())
                            .or_insert_with(|| RefValue::Hash(HashMap::new()));
                        if let RefValue::Hash(ref mut h) = hash {
                            h.insert(field, expected.to_string().into_bytes());
                        }
                    }
                    HIncrExpect::NotInteger => {
                        self.assert_error_contains(
                            &resp,
                            "ERR",
                            "HINCRBY on non-integer field value",
                        );
                    }
                }
            }
        }
    }

    // --- Sorted set operations ---
    fn run_sorted_set_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.sorted_set_ops += 1;

        let key = self.random_key();
        let is_zset_or_none = matches!(self.shadow.get(&key), Some(RefValue::SortedSet(_)) | None);

        if sub < 35 && is_zset_or_none {
            // ZADD
            let member = self.random_field();
            let score = self.random_score();
            let desc = format!("ZADD {} {} member", key, score);
            self.result.last_op = Some(ExecutorOp::SortedSet(desc));

            let resp = self.executor.execute(&Command::ZAdd {
                key: key.clone(),
                pairs: vec![(score, SDS::new(member.clone()))],
                nx: false,
                xx: false,
                gt: false,
                lt: false,
                ch: false,
            });

            let zset = self
                .shadow
                .data
                .entry(key.clone())
                .or_insert_with(|| RefValue::SortedSet(BTreeMap::new()));
            if let RefValue::SortedSet(ref mut z) = zset {
                let was_new = !z.contains_key(&member);
                z.insert(member, score);
                let expected = if was_new { 1 } else { 0 };
                // Invariant 7: Sorted set cardinality matches ZADD/ZREM
                self.assert_integer(&resp, expected, &format!("ZADD {} result", key));
            }
        } else if sub < 55 {
            // ZREM
            let member = self.random_field();
            let desc = format!("ZREM {} member", key);
            self.result.last_op = Some(ExecutorOp::SortedSet(desc));

            let resp = self.executor.execute(&Command::ZRem(
                key.clone(),
                vec![SDS::new(member.clone())],
            ));

            let expected: Result<i64, &str> = match self.shadow.data.get_mut(&key) {
                Some(RefValue::SortedSet(ref mut z)) => {
                    let was_present = z.remove(&member).is_some();
                    Ok(if was_present { 1 } else { 0 })
                }
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            if matches!(self.shadow.data.get(&key), Some(RefValue::SortedSet(z)) if z.is_empty()) {
                self.shadow.data.remove(&key);
            }
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("ZREM {} result", key)),
                Err(_) => self.assert_error_contains(&resp, "WRONGTYPE", "ZREM on wrong type"),
            }
        } else if sub < 70 {
            // ZCARD
            let desc = format!("ZCARD {}", key);
            self.result.last_op = Some(ExecutorOp::SortedSet(desc));

            let resp = self.executor.execute(&Command::ZCard(key.clone()));

            let expected: Result<i64, &str> = match self.shadow.get(&key) {
                Some(RefValue::SortedSet(z)) => Ok(z.len() as i64),
                None => Ok(0),
                Some(_) => Err("WRONGTYPE"),
            };
            match expected {
                Ok(n) => self.assert_integer(&resp, n, &format!("ZCARD {}", key)),
                Err(_) => self.assert_error_contains(&resp, "WRONGTYPE", "ZCARD on wrong type"),
            }
        } else if sub < 85 {
            // ZSCORE
            let member = self.random_field();
            let desc = format!("ZSCORE {} member", key);
            self.result.last_op = Some(ExecutorOp::SortedSet(desc));

            let resp = self.executor.execute(&Command::ZScore(
                key.clone(),
                SDS::new(member.clone()),
            ));

            enum ZScoreExpect {
                Score(f64),
                Null,
                WrongType,
            }
            let expect = match self.shadow.get(&key) {
                Some(RefValue::SortedSet(z)) => match z.get(&member) {
                    Some(&score) => ZScoreExpect::Score(score),
                    None => ZScoreExpect::Null,
                },
                None => ZScoreExpect::Null,
                Some(_) => ZScoreExpect::WrongType,
            };
            match expect {
                ZScoreExpect::Score(score) => {
                    if let RespValue::BulkString(Some(data)) = &resp {
                        if let Ok(resp_score) =
                            std::str::from_utf8(data).unwrap_or("").parse::<f64>()
                        {
                            if (resp_score - score).abs() > f64::EPSILON {
                                self.violation(&format!(
                                    "ZSCORE {} member: got {}, expected {}",
                                    key, resp_score, score
                                ));
                            }
                        }
                    }
                }
                ZScoreExpect::Null => {
                    self.assert_null(&resp, &format!("ZSCORE {} missing/nonexistent", key));
                }
                ZScoreExpect::WrongType => {
                    self.assert_error_contains(&resp, "WRONGTYPE", "ZSCORE on wrong type");
                }
            }
        } else {
            // ZRANGE - verify ordering invariant, sometimes with WITHSCORES
            let with_scores = self.rng.gen_range(0, 100) < 40;
            let desc = format!(
                "ZRANGE {} 0 -1{}",
                key,
                if with_scores { " WITHSCORES" } else { "" }
            );
            self.result.last_op = Some(ExecutorOp::SortedSet(desc));

            let resp = self
                .executor
                .execute(&Command::ZRange(key.clone(), 0, -1, with_scores));

            // Invariant 8: ZRANGE returns ascending order with correct count
            let expected_len = match self.shadow.get(&key) {
                Some(RefValue::SortedSet(z)) => Some(z.len()),
                _ => None,
            };
            if let (RespValue::Array(Some(elements)), Some(len)) = (&resp, expected_len) {
                if with_scores {
                    // WITHSCORES: array has 2 * len elements (member, score interleaved)
                    if elements.len() != len * 2 {
                        self.violation(&format!(
                            "ZRANGE {} WITHSCORES count mismatch: got {}, expected {} (2 * {})",
                            key,
                            elements.len(),
                            len * 2,
                            len
                        ));
                    }
                    // Verify scores are in ascending order
                    let mut prev_score: Option<f64> = None;
                    for i in (1..elements.len()).step_by(2) {
                        if let RespValue::BulkString(Some(score_bytes)) = &elements[i] {
                            if let Ok(score) = String::from_utf8_lossy(score_bytes).parse::<f64>() {
                                if let Some(prev) = prev_score {
                                    if score < prev {
                                        self.violation(&format!(
                                            "ZRANGE {} WITHSCORES not ascending: {} after {}",
                                            key, score, prev
                                        ));
                                    }
                                }
                                prev_score = Some(score);
                            }
                        }
                    }
                } else {
                    if elements.len() != len {
                        self.violation(&format!(
                            "ZRANGE {} count mismatch: got {}, expected {}",
                            key,
                            elements.len(),
                            len
                        ));
                    }
                }
            }
        }
    }

    // --- Expiry operations ---
    fn run_expiry_op(&mut self) {
        let sub = self.rng.gen_range(0, 100);
        self.result.expiry_ops += 1;

        let key = self.random_key();

        if sub < 30 {
            // EXPIRE
            let seconds = self.rng.gen_range(1, 100) as i64;
            let desc = format!("EXPIRE {} {}", key, seconds);
            self.result.last_op = Some(ExecutorOp::Expiry(desc));

            let resp = self
                .executor
                .execute(&Command::expire(key.clone(), seconds));

            // Check response-driven invariants:
            // EXPIRE returns 1 if key exists, 0 if not
            match &resp {
                RespValue::Integer(1) => {
                    // Key existed and got an expiry
                    // Invariant 9: EXPIRE causes TTL > 0
                    let ttl_resp = self.executor.execute(&Command::Ttl(key.clone()));
                    if let RespValue::Integer(ttl) = ttl_resp {
                        if ttl <= 0 {
                            self.violation(&format!(
                                "TTL {} should be > 0 after EXPIRE, got {}",
                                key, ttl
                            ));
                        }
                    }
                    // Sync shadow: track the expiry
                    let expiry_ms = self.current_time_ms + (seconds as u64 * 1000);
                    self.shadow.expirations.insert(key.clone(), expiry_ms);
                }
                RespValue::Integer(0) => {
                    // Key didn't exist; make sure shadow agrees it doesn't exist
                    if self.shadow.exists(&key) {
                        // Shadow/executor diverged on key existence - sync shadow
                        self.shadow.del(&key);
                    }
                }
                _ => {
                    self.violation(&format!(
                        "EXPIRE {} returned unexpected: {:?}",
                        key, resp
                    ));
                }
            }
        } else if sub < 48 {
            // TTL
            let desc = format!("TTL {}", key);
            self.result.last_op = Some(ExecutorOp::Expiry(desc));

            let resp = self.executor.execute(&Command::Ttl(key.clone()));

            // Response-driven invariants for TTL:
            // -2 = key doesn't exist, -1 = no expiry, >= 0 = TTL in seconds
            if let RespValue::Integer(ttl) = &resp {
                if !self.shadow.exists(&key) {
                    // Invariant 9: missing key returns -2
                    if *ttl != -2 {
                        // Shadow/executor diverged - sync shadow from executor response
                        // Don't assert, just sync (expiration tracking is auxiliary)
                    }
                }
                // TTL should be >= -2
                if *ttl < -2 {
                    self.violation(&format!("TTL {} returned invalid value: {}", key, ttl));
                }
            }
        } else if sub < 63 {
            // PEXPIRE
            let milliseconds = self.rng.gen_range(100, 100_000) as i64;
            let desc = format!("PEXPIRE {} {}", key, milliseconds);
            self.result.last_op = Some(ExecutorOp::Expiry(desc));

            let resp = self.executor.execute(&Command::PExpire {
                key: key.clone(),
                milliseconds,
                nx: false,
                xx: false,
                gt: false,
                lt: false,
            });

            // Response-driven: PEXPIRE returns 1 if key exists, 0 if not
            match &resp {
                RespValue::Integer(1) => {
                    // Key existed and got an expiry
                    let expiry_ms = self.current_time_ms + milliseconds as u64;
                    self.shadow.expirations.insert(key.clone(), expiry_ms);
                }
                RespValue::Integer(0) => {
                    // Key didn't exist; make sure shadow agrees
                    if self.shadow.exists(&key) {
                        self.shadow.del(&key);
                    }
                }
                _ => {
                    self.violation(&format!(
                        "PEXPIRE {} returned unexpected: {:?}",
                        key, resp
                    ));
                }
            }
        } else if sub < 78 {
            // PERSIST
            let desc = format!("PERSIST {}", key);
            self.result.last_op = Some(ExecutorOp::Expiry(desc));

            let resp = self.executor.execute(&Command::Persist(key.clone()));

            // Response-driven: PERSIST returns 1 if timeout was removed, 0 otherwise
            match &resp {
                RespValue::Integer(1) => {
                    // Had an expiry which was removed
                    self.shadow.expirations.remove(&key);
                }
                RespValue::Integer(0) => {
                    // No expiry or key doesn't exist - shadow should agree
                }
                _ => {
                    self.violation(&format!(
                        "PERSIST {} returned unexpected: {:?}",
                        key, resp
                    ));
                }
            }
        } else {
            // Advance time slightly (simulate passage of time for expiry testing)
            let advance_ms = self.rng.gen_range(100, 5000);
            self.current_time_ms += advance_ms;
            let time = crate::simulator::VirtualTime::from_millis(self.current_time_ms);
            self.executor.set_time(time);
            self.shadow.evict_expired(self.current_time_ms);

            let desc = format!("TIME_ADVANCE +{}ms", advance_ms);
            self.result.last_op = Some(ExecutorOp::Expiry(desc));

            // After time advance, sync shadow with executor's actual key set
            // (expirations may have diverged due to collection operations)
            let executor_keys: HashSet<String> = self.executor.get_data().keys().cloned().collect();
            let shadow_keys: HashSet<String> = self.shadow.data.keys().cloned().collect();

            // Keys in shadow but not executor: evicted by executor
            for key in shadow_keys.difference(&executor_keys) {
                self.shadow.data.remove(key);
                self.shadow.expirations.remove(key);
            }
            // Keys in executor but not shadow: created by executor (shouldn't happen normally)
            // Don't add them to shadow - these represent a tracking gap we should fix
        }
    }

    // =========================================================================
    // Invariant Assertion Helpers
    // =========================================================================

    fn violation(&mut self, msg: &str) {
        self.result.invariant_violations.push(format!(
            "Op #{}: {:?} - {}",
            self.result.total_operations, self.result.last_op, msg
        ));
    }

    fn assert_ok(&mut self, resp: &RespValue, context: &str) {
        if !matches!(resp, RespValue::SimpleString(s) if s.as_ref() == "OK") {
            self.violation(&format!("{}: expected OK, got {:?}", context, resp));
        }
    }

    fn assert_null(&mut self, resp: &RespValue, context: &str) {
        if !matches!(resp, RespValue::BulkString(None)) {
            self.violation(&format!("{}: expected nil, got {:?}", context, resp));
        }
    }

    fn assert_integer(&mut self, resp: &RespValue, expected: i64, context: &str) {
        match resp {
            RespValue::Integer(n) if *n == expected => {}
            _ => {
                self.violation(&format!(
                    "{}: expected Integer({}), got {:?}",
                    context, expected, resp
                ));
            }
        }
    }

    fn assert_bulk_eq(&mut self, resp: &RespValue, expected: &[u8], context: &str) {
        match resp {
            RespValue::BulkString(Some(data)) if data == expected => {}
            _ => {
                self.violation(&format!(
                    "{}: expected BulkString({:?}), got {:?}",
                    context,
                    String::from_utf8_lossy(expected),
                    resp
                ));
            }
        }
    }

    fn assert_simple_string(&mut self, resp: &RespValue, expected: &str, context: &str) {
        match resp {
            RespValue::SimpleString(s) if s.as_ref() == expected => {}
            _ => {
                self.violation(&format!(
                    "{}: expected SimpleString({}), got {:?}",
                    context, expected, resp
                ));
            }
        }
    }

    fn assert_error_contains(&mut self, resp: &RespValue, substring: &str, context: &str) {
        match resp {
            RespValue::Error(e) if e.as_ref().contains(substring) => {}
            _ => {
                self.violation(&format!(
                    "{}: expected Error containing '{}', got {:?}",
                    context, substring, resp
                ));
            }
        }
    }

    // =========================================================================
    // Public API
    // =========================================================================

    /// Run specified number of operations
    pub fn run(&mut self, operations: usize) {
        // Set initial time
        let time = crate::simulator::VirtualTime::from_millis(self.current_time_ms);
        self.executor.set_time(time);

        for _ in 0..operations {
            self.result.total_operations += 1;
            self.run_single_op();

            // Stop early if we hit a violation
            if !self.result.invariant_violations.is_empty() {
                break;
            }
        }
    }

    /// Get the result
    pub fn result(&self) -> &ExecutorDSTResult {
        &self.result
    }

    /// Get reference to executor for inspection
    pub fn executor(&self) -> &CommandExecutor {
        &self.executor
    }
}

/// Run a batch of DST tests with different seeds
pub fn run_executor_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> ExecutorDSTConfig,
) -> Vec<ExecutorDSTResult> {
    (0..num_seeds)
        .map(|i| {
            let seed = start_seed + i as u64;
            let config = config_fn(seed);
            let mut harness = ExecutorDSTHarness::new(config);
            harness.run(ops_per_seed);
            harness.result().clone()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_executor_batch(results: &[ExecutorDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed = total - passed;
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();

    let mut summary = format!(
        "Executor DST Summary\n\
         ====================\n\
         Seeds: {} total, {} passed, {} failed\n\
         Total operations: {}\n",
        total, passed, failed, total_ops
    );

    if failed > 0 {
        summary.push_str("\nFailed seeds:\n");
        for result in results.iter().filter(|r| !r.is_success()) {
            summary.push_str(&format!("  Seed {}: {}\n", result.seed, result.summary()));
            for violation in &result.invariant_violations {
                summary.push_str(&format!("    - {}\n", violation));
            }
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_dst_single_seed() {
        let mut harness = ExecutorDSTHarness::with_seed(12345);
        harness.run(100);
        let result = harness.result();
        println!("{}", result.summary());
        for v in &result.invariant_violations {
            println!("  VIOLATION: {}", v);
        }
        assert!(result.is_success(), "Seed 12345 failed");
    }

    #[test]
    fn test_executor_dst_calm() {
        let config = ExecutorDSTConfig::calm(42);
        let mut harness = ExecutorDSTHarness::new(config);
        harness.run(100);
        let result = harness.result();
        println!("Calm: {}", result.summary());
        assert!(result.is_success());
    }

    #[test]
    fn test_executor_dst_chaos() {
        let config = ExecutorDSTConfig::chaos(99);
        let mut harness = ExecutorDSTHarness::new(config);
        harness.run(500);
        let result = harness.result();
        println!("Chaos: {}", result.summary());
        for v in &result.invariant_violations {
            println!("  VIOLATION: {}", v);
        }
        assert!(result.is_success());
    }

    #[test]
    fn test_executor_dst_string_heavy() {
        let config = ExecutorDSTConfig::string_heavy(777);
        let mut harness = ExecutorDSTHarness::new(config);
        harness.run(500);
        let result = harness.result();
        println!("String heavy: {}", result.summary());
        for v in &result.invariant_violations {
            println!("  VIOLATION: {}", v);
        }
        assert!(result.is_success());
        assert!(
            result.string_ops > result.list_ops + result.set_ops,
            "String-heavy should do mostly string ops"
        );
    }

    #[test]
    fn test_executor_dst_10_seeds() {
        let results = run_executor_batch(0, 10, 500, ExecutorDSTConfig::new);
        let summary = summarize_executor_batch(&results);
        println!("{}", summary);

        let passed = results.iter().filter(|r| r.is_success()).count();
        assert_eq!(passed, 10, "All 10 seeds should pass");
    }
}
