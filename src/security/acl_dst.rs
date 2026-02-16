//! Deterministic Simulation Testing for the ACL system.
//!
//! Shadow-state testing harness for AclManager that enables:
//! - Deterministic random operation generation (SETUSER, DELUSER, AUTH, permission checks)
//! - Invariant checking after each operation
//! - Seed-based reproducibility for debugging
//!
//! The shadow state is the specification — the real AclManager must match it for all seeds.

use crate::io::simulation::SimulatedRng;
use crate::io::Rng;
use crate::security::acl::{AclCommandHandler, AclManager, CommandCategory};
use std::collections::{HashMap, HashSet};

/// Configuration for ACL DST
#[derive(Debug, Clone)]
pub struct AclDSTConfig {
    /// Random seed for reproducibility
    pub seed: u64,
    /// Number of operations per run
    pub num_operations: usize,
    /// Number of unique usernames in the pool
    pub num_users: usize,
    /// Number of unique keys in the pool
    pub key_pool_size: usize,
    /// Number of unique passwords in the pool
    pub password_pool_size: usize,
}

impl Default for AclDSTConfig {
    fn default() -> Self {
        AclDSTConfig {
            seed: 0,
            num_operations: 500,
            num_users: 10,
            key_pool_size: 20,
            password_pool_size: 8,
        }
    }
}

impl AclDSTConfig {
    pub fn new(seed: u64) -> Self {
        AclDSTConfig {
            seed,
            ..Default::default()
        }
    }

    /// Small user pool (more collisions on SETUSER/DELUSER)
    pub fn small_users(seed: u64) -> Self {
        AclDSTConfig {
            seed,
            num_users: 4,
            key_pool_size: 8,
            password_pool_size: 4,
            ..Default::default()
        }
    }

    /// Large user pool
    pub fn large_users(seed: u64) -> Self {
        AclDSTConfig {
            seed,
            num_users: 30,
            key_pool_size: 50,
            password_pool_size: 15,
            ..Default::default()
        }
    }

    /// High churn (lots of user creation/deletion)
    pub fn high_churn(seed: u64) -> Self {
        AclDSTConfig {
            seed,
            num_users: 8,
            num_operations: 1000,
            key_pool_size: 15,
            password_pool_size: 6,
        }
    }
}

/// Shadow representation of a user for verification
#[derive(Debug, Clone)]
struct ShadowUser {
    enabled: bool,
    nopass: bool,
    password_hashes: HashSet<String>,
    commands_allow_all: bool,
    allowed_commands: HashSet<String>,
    denied_commands: HashSet<String>,
    allowed_categories: HashSet<CommandCategory>,
    denied_categories: HashSet<CommandCategory>,
    keys_allow_all: bool,
    key_patterns: Vec<String>,
}

impl ShadowUser {
    /// Create shadow state matching AclUser::new()
    fn new_default_off() -> Self {
        ShadowUser {
            enabled: false,
            nopass: false,
            password_hashes: HashSet::new(),
            commands_allow_all: false,
            allowed_commands: HashSet::new(),
            denied_commands: HashSet::new(),
            allowed_categories: HashSet::new(),
            denied_categories: HashSet::new(),
            keys_allow_all: false,
            key_patterns: Vec::new(),
        }
    }

    /// Create shadow state matching AclUser::default_user()
    fn default_user() -> Self {
        ShadowUser {
            enabled: true,
            nopass: true,
            password_hashes: HashSet::new(),
            commands_allow_all: true,
            allowed_commands: HashSet::new(),
            denied_commands: HashSet::new(),
            allowed_categories: HashSet::new(),
            denied_categories: HashSet::new(),
            keys_allow_all: true,
            key_patterns: Vec::new(),
        }
    }

    /// Check if a command is permitted (mirrors CommandPermissions::is_command_permitted)
    fn is_command_permitted(&self, command: &str) -> bool {
        let cmd = command.to_uppercase();

        // Explicit deny takes precedence
        if self.denied_commands.contains(&cmd) {
            return false;
        }

        // Explicit allow
        if self.allowed_commands.contains(&cmd) {
            return true;
        }

        // Check denied categories
        for cat in &self.denied_categories {
            if cat.commands().contains(&cmd.as_str()) {
                return false;
            }
        }

        // Check allowed categories
        for cat in &self.allowed_categories {
            if *cat == CommandCategory::All {
                return true;
            }
            if cat.commands().contains(&cmd.as_str()) {
                return true;
            }
        }

        self.commands_allow_all
    }

    /// Check if a key is permitted (mirrors KeyPatterns::is_key_permitted)
    fn is_key_permitted(&self, key: &str) -> bool {
        if self.keys_allow_all {
            return true;
        }
        for pattern in &self.key_patterns {
            if glob_match(pattern, key) {
                return true;
            }
        }
        false
    }

    /// Apply a rule to this shadow user (mirrors apply_rule)
    fn apply_rule(&mut self, rule: &str) {
        let rule = rule.trim();
        match rule {
            "on" => self.enabled = true,
            "off" => self.enabled = false,
            "nopass" => self.nopass = true,
            "resetpass" => {
                self.password_hashes.clear();
                self.nopass = false;
            }
            "allcommands" | "+@all" => {
                self.commands_allow_all = true;
                self.allowed_categories.insert(CommandCategory::All);
            }
            "nocommands" | "-@all" => {
                self.commands_allow_all = false;
                self.allowed_categories.clear();
                self.allowed_commands.clear();
            }
            "allkeys" | "~*" => {
                self.keys_allow_all = true;
            }
            "resetkeys" => {
                self.keys_allow_all = false;
                self.key_patterns.clear();
            }
            "reset" => {
                *self = Self::new_default_off();
            }
            _ => {
                if let Some(rest) = rule.strip_prefix('>') {
                    let hash = AclManager::hash_password(rest);
                    self.password_hashes.insert(hash);
                } else if let Some(rest) = rule.strip_prefix('<') {
                    let hash = AclManager::hash_password(rest);
                    self.password_hashes.remove(&hash);
                } else if let Some(rest) = rule.strip_prefix('#') {
                    self.password_hashes.insert(rest.to_string());
                } else if let Some(rest) = rule.strip_prefix('+') {
                    if let Some(cat) = rest.strip_prefix('@') {
                        if let Some(category) = CommandCategory::from_str(cat) {
                            self.allowed_categories.insert(category);
                        }
                    } else {
                        let cmd = rest.to_uppercase();
                        self.denied_commands.remove(&cmd);
                        self.allowed_commands.insert(cmd);
                    }
                } else if let Some(rest) = rule.strip_prefix('-') {
                    if let Some(cat) = rest.strip_prefix('@') {
                        if let Some(category) = CommandCategory::from_str(cat) {
                            self.allowed_categories.remove(&category);
                            self.denied_categories.insert(category);
                        }
                    } else {
                        let cmd = rest.to_uppercase();
                        self.allowed_commands.remove(&cmd);
                        self.denied_commands.insert(cmd);
                    }
                } else if let Some(rest) = rule.strip_prefix('~') {
                    self.key_patterns.push(rest.to_string());
                }
            }
        }
    }

    /// Check if a password authenticates (mirrors AclUser::verify_password)
    fn verify_password(&self, password: &str) -> bool {
        if self.nopass {
            return true;
        }
        let hash = AclManager::hash_password(password);
        self.password_hashes.contains(&hash)
    }
}

/// Simple glob pattern matching (mirrors patterns.rs::glob_match)
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    glob_match_impl(&pattern_chars, &text_chars)
}

fn glob_match_impl(pattern: &[char], text: &[char]) -> bool {
    let mut p = 0;
    let mut t = 0;
    let mut star_p = None;
    let mut star_t = None;

    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                '?' => {
                    p += 1;
                    t += 1;
                    continue;
                }
                '*' => {
                    star_p = Some(p);
                    star_t = Some(t);
                    p += 1;
                    continue;
                }
                c if c == text[t] => {
                    p += 1;
                    t += 1;
                    continue;
                }
                _ => {}
            }
        }
        if let (Some(sp), Some(st)) = (star_p, star_t) {
            p = sp + 1;
            star_t = Some(st + 1);
            t = st + 1;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// Operation types for logging
#[derive(Debug, Clone)]
pub enum AclOp {
    SetUser {
        username: String,
        rules: Vec<String>,
    },
    DelUser {
        username: String,
    },
    Auth {
        username: String,
        password: String,
    },
    CheckCommand {
        username: String,
        command: String,
        keys: Vec<String>,
    },
    CheckKey {
        username: String,
        key: String,
    },
}

/// Result of an ACL DST run
#[derive(Debug, Clone)]
pub struct AclDSTResult {
    pub seed: u64,
    pub total_operations: u64,
    pub set_users: u64,
    pub del_users: u64,
    pub auths: u64,
    pub auth_mismatches: u64,
    pub command_checks: u64,
    pub command_check_mismatches: u64,
    pub key_checks: u64,
    pub key_check_mismatches: u64,
    pub invariant_violations: Vec<String>,
    pub last_op: Option<AclOp>,
}

impl AclDSTResult {
    fn new(seed: u64) -> Self {
        AclDSTResult {
            seed,
            total_operations: 0,
            set_users: 0,
            del_users: 0,
            auths: 0,
            auth_mismatches: 0,
            command_checks: 0,
            command_check_mismatches: 0,
            key_checks: 0,
            key_check_mismatches: 0,
            invariant_violations: Vec::new(),
            last_op: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.invariant_violations.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "Seed {}: {} ops (setuser:{}, deluser:{}, auth:{}, cmd_checks:{}, key_checks:{}), {} violations",
            self.seed,
            self.total_operations,
            self.set_users,
            self.del_users,
            self.auths,
            self.command_checks,
            self.key_checks,
            self.invariant_violations.len()
        )
    }
}

/// DST harness for ACL system
pub struct AclDSTHarness {
    config: AclDSTConfig,
    rng: SimulatedRng,
    /// The real AclManager under test
    manager: AclManager,
    /// Shadow state — the specification
    shadow: HashMap<String, ShadowUser>,
    result: AclDSTResult,
    /// Pool of usernames (excluding "default")
    user_pool: Vec<String>,
    /// Pool of passwords
    password_pool: Vec<String>,
    /// Pool of keys
    key_pool: Vec<String>,
}

/// Available categories for random selection
const RANDOM_CATEGORIES: &[&str] = &[
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

/// Commands to test permission checks against
const TEST_COMMANDS: &[&str] = &[
    "GET", "SET", "DEL", "MGET", "MSET", "HGET", "HSET", "LPUSH", "RPOP", "SADD", "SREM",
    "ZADD", "ZREM", "KEYS", "SCAN", "INFO", "PING", "AUTH", "FLUSHALL", "CONFIG",
];

impl AclDSTHarness {
    pub fn new(config: AclDSTConfig) -> Self {
        let rng = SimulatedRng::new(config.seed);

        let user_pool: Vec<String> = (0..config.num_users)
            .map(|i| format!("user{}", i))
            .collect();
        let password_pool: Vec<String> = (0..config.password_pool_size)
            .map(|i| format!("pass{}", i))
            .collect();
        let key_pool: Vec<String> = (0..config.key_pool_size)
            .map(|i| format!("key:{}", i))
            .collect();

        let mut shadow = HashMap::new();
        shadow.insert("default".to_string(), ShadowUser::default_user());

        AclDSTHarness {
            result: AclDSTResult::new(config.seed),
            config,
            rng,
            manager: AclManager::new(),
            shadow,
            user_pool,
            password_pool,
            key_pool,
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self::new(AclDSTConfig::new(seed))
    }

    fn random_username(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.user_pool.len() as u64) as usize;
        self.user_pool[idx].clone()
    }

    fn random_password(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.password_pool.len() as u64) as usize;
        self.password_pool[idx].clone()
    }

    fn random_key(&mut self) -> String {
        let idx = self.rng.gen_range(0, self.key_pool.len() as u64) as usize;
        self.key_pool[idx].clone()
    }

    fn random_command(&mut self) -> String {
        let idx = self.rng.gen_range(0, TEST_COMMANDS.len() as u64) as usize;
        TEST_COMMANDS[idx].to_string()
    }

    fn random_category(&mut self) -> String {
        let idx = self.rng.gen_range(0, RANDOM_CATEGORIES.len() as u64) as usize;
        RANDOM_CATEGORIES[idx].to_string()
    }

    /// Generate a random set of rules for SETUSER
    fn random_rules(&mut self) -> Vec<String> {
        let num_rules = self.rng.gen_range(1, 6) as usize;
        let mut rules = Vec::with_capacity(num_rules);

        for _ in 0..num_rules {
            let rule_type = self.rng.gen_range(0, 10);
            let rule = match rule_type {
                0 => "on".to_string(),
                1 => "off".to_string(),
                2 => {
                    let pw = self.random_password();
                    format!(">{}", pw)
                }
                3 => "nopass".to_string(),
                4 => {
                    let cat = self.random_category();
                    format!("+@{}", cat)
                }
                5 => {
                    let cat = self.random_category();
                    format!("-@{}", cat)
                }
                6 => "+@all".to_string(),
                7 => "~*".to_string(),
                8 => {
                    // Key pattern — pick a prefix from key pool
                    let key = self.random_key();
                    let prefix = key.split(':').next().unwrap_or("k");
                    format!("~{}:*", prefix)
                }
                _ => {
                    // Allow/deny specific command
                    let cmd = self.random_command();
                    if self.rng.gen_bool(0.5) {
                        format!("+{}", cmd.to_lowercase())
                    } else {
                        format!("-{}", cmd.to_lowercase())
                    }
                }
            };
            rules.push(rule);
        }

        rules
    }

    fn run_single_op(&mut self) {
        let op_type = self.rng.gen_range(0, 100);

        if op_type < 30 {
            // SETUSER (30%)
            self.op_setuser();
        } else if op_type < 40 {
            // DELUSER (10%)
            self.op_deluser();
        } else if op_type < 60 {
            // AUTH (20%)
            self.op_auth();
        } else if op_type < 85 {
            // Command permission check (25%)
            self.op_check_command();
        } else {
            // Key permission check (15%)
            self.op_check_key();
        }

        self.result.total_operations += 1;

        // Verify user list matches after every operation
        if let Err(violation) = self.check_user_list_invariant() {
            self.result.invariant_violations.push(format!(
                "Op #{}: {:?} - {}",
                self.result.total_operations, self.result.last_op, violation
            ));
        }
    }

    fn op_setuser(&mut self) {
        let username = self.random_username();
        let rules = self.random_rules();

        self.result.last_op = Some(AclOp::SetUser {
            username: username.clone(),
            rules: rules.clone(),
        });
        self.result.set_users += 1;

        // Apply to real manager
        let rule_refs: Vec<&str> = rules.iter().map(|s| s.as_str()).collect();
        let real_result = AclCommandHandler::handle_setuser(&mut self.manager, &username, &rule_refs);

        // Apply to shadow
        let shadow_user = self
            .shadow
            .entry(username.clone())
            .or_insert_with(ShadowUser::new_default_off);
        for rule in &rules {
            shadow_user.apply_rule(rule);
        }

        // Real should succeed (all our generated rules are valid)
        if let Err(e) = real_result {
            self.result.invariant_violations.push(format!(
                "SETUSER '{}' with rules {:?} failed unexpectedly: {}",
                username, rules, e
            ));
        }
    }

    fn op_deluser(&mut self) {
        let username = self.random_username();

        self.result.last_op = Some(AclOp::DelUser {
            username: username.clone(),
        });
        self.result.del_users += 1;

        // "default" can't be deleted — handled by both real and shadow
        if username == "default" {
            return;
        }

        let shadow_existed = self.shadow.remove(&username).is_some();
        let real_result = AclCommandHandler::handle_deluser(&mut self.manager, &[username.as_str()]);

        match real_result {
            Ok(count) => {
                let real_existed = count > 0;
                if shadow_existed != real_existed {
                    self.result.invariant_violations.push(format!(
                        "DELUSER '{}': shadow_existed={}, real_existed={}",
                        username, shadow_existed, real_existed
                    ));
                }
            }
            Err(e) => {
                self.result.invariant_violations.push(format!(
                    "DELUSER '{}' failed unexpectedly: {}",
                    username, e
                ));
            }
        }
    }

    fn op_auth(&mut self) {
        let username = self.random_username();
        let password = self.random_password();

        self.result.last_op = Some(AclOp::Auth {
            username: username.clone(),
            password: password.clone(),
        });
        self.result.auths += 1;

        // Shadow: compute expected auth result
        let shadow_result = match self.shadow.get(&username) {
            None => false,     // User doesn't exist
            Some(su) => {
                if !su.enabled {
                    false       // User disabled
                } else {
                    su.verify_password(&password)
                }
            }
        };

        // Real: attempt authentication
        let real_result = self.manager.authenticate(&username, &password);
        let real_success = real_result.is_ok();

        if shadow_result != real_success {
            self.result.auth_mismatches += 1;
            self.result.invariant_violations.push(format!(
                "AUTH mismatch for '{}' with password '{}': shadow={}, real={}",
                username, password, shadow_result, real_success
            ));
        }
    }

    fn op_check_command(&mut self) {
        let username = self.random_username();
        let command = self.random_command();
        let num_keys = self.rng.gen_range(0, 3) as usize;
        let keys: Vec<String> = (0..num_keys).map(|_| self.random_key()).collect();

        self.result.last_op = Some(AclOp::CheckCommand {
            username: username.clone(),
            command: command.clone(),
            keys: keys.clone(),
        });
        self.result.command_checks += 1;

        // Shadow: compute expected result
        let shadow_permitted = match self.shadow.get(&username) {
            None => false,
            Some(su) => {
                if !su.enabled {
                    false
                } else {
                    let cmd_ok = su.is_command_permitted(&command);
                    let keys_ok = keys.iter().all(|k| su.is_key_permitted(k));
                    cmd_ok && keys_ok
                }
            }
        };

        // Real: check via AclManager
        let real_user = self.manager.get_user(&username);
        let real_permitted = match &real_user {
            None => false,
            Some(u) => {
                if !u.enabled {
                    false
                } else {
                    let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                    self.manager
                        .check_command(Some(u.as_ref()), &command, &key_refs)
                        .is_ok()
                }
            }
        };

        if shadow_permitted != real_permitted {
            self.result.command_check_mismatches += 1;
            self.result.invariant_violations.push(format!(
                "Command check mismatch for user='{}' cmd='{}' keys={:?}: shadow={}, real={}",
                username, command, keys, shadow_permitted, real_permitted
            ));
        }
    }

    fn op_check_key(&mut self) {
        let username = self.random_username();
        let key = self.random_key();

        self.result.last_op = Some(AclOp::CheckKey {
            username: username.clone(),
            key: key.clone(),
        });
        self.result.key_checks += 1;

        // Shadow: check key permission
        let shadow_permitted = match self.shadow.get(&username) {
            None => false,
            Some(su) => su.is_key_permitted(&key),
        };

        // Real: check via AclManager
        let real_user = self.manager.get_user(&username);
        let real_permitted = match &real_user {
            None => false,
            Some(u) => u.keys.is_key_permitted(&key),
        };

        if shadow_permitted != real_permitted {
            self.result.key_check_mismatches += 1;
            self.result.invariant_violations.push(format!(
                "Key check mismatch for user='{}' key='{}': shadow={}, real={}",
                username, key, shadow_permitted, real_permitted
            ));
        }
    }

    /// Verify that the set of users in shadow matches the real manager
    fn check_user_list_invariant(&self) -> Result<(), String> {
        let real_names: HashSet<&str> = self.manager.user_names().into_iter().collect();
        let shadow_names: HashSet<&str> = self.shadow.keys().map(|s| s.as_str()).collect();

        if real_names != shadow_names {
            let missing_in_real: Vec<_> = shadow_names.difference(&real_names).collect();
            let extra_in_real: Vec<_> = real_names.difference(&shadow_names).collect();
            return Err(format!(
                "User list mismatch: missing_in_real={:?}, extra_in_real={:?}",
                missing_in_real, extra_in_real
            ));
        }

        // Also verify invariants on the real manager
        self.manager.verify_invariants();

        Ok(())
    }

    pub fn run(&mut self, operations: usize) {
        for _ in 0..operations {
            self.run_single_op();
            if !self.result.invariant_violations.is_empty() {
                break;
            }
        }
    }

    pub fn result(&self) -> &AclDSTResult {
        &self.result
    }
}

/// Run a batch of DST tests
pub fn run_acl_batch(
    start_seed: u64,
    num_seeds: usize,
    ops_per_seed: usize,
    config_fn: fn(u64) -> AclDSTConfig,
) -> Vec<AclDSTResult> {
    (0..num_seeds)
        .map(|i| {
            let seed = start_seed + i as u64;
            let config = config_fn(seed);
            let ops = ops_per_seed.max(config.num_operations);
            let mut harness = AclDSTHarness::new(config);
            harness.run(ops);
            harness.result().clone()
        })
        .collect()
}

/// Summarize batch results
pub fn summarize_acl_batch(results: &[AclDSTResult]) -> String {
    let total = results.len();
    let passed = results.iter().filter(|r| r.is_success()).count();
    let failed = total - passed;
    let total_ops: u64 = results.iter().map(|r| r.total_operations).sum();

    let mut summary = format!(
        "ACL DST Summary\n\
         ===============\n\
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
    use crate::security::acl::{apply_rule, AclUser, KeyPatterns};

    #[test]
    fn test_acl_dst_single_seed() {
        let mut harness = AclDSTHarness::with_seed(12345);
        harness.run(200);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success(), "Seed 12345 failed: {:?}", result.invariant_violations);
    }

    #[test]
    fn test_acl_dst_small_users() {
        let config = AclDSTConfig::small_users(42);
        let mut harness = AclDSTHarness::new(config);
        harness.run(500);
        let result = harness.result();
        println!("{}", result.summary());
        assert!(result.is_success(), "Small users failed: {:?}", result.invariant_violations);
    }

    #[test]
    fn test_acl_dst_10_seeds() {
        let results = run_acl_batch(0, 10, 500, AclDSTConfig::new);
        let summary = summarize_acl_batch(&results);
        println!("{}", summary);

        let passed = results.iter().filter(|r| r.is_success()).count();
        assert_eq!(passed, 10, "All 10 seeds should pass");
    }

    #[test]
    fn test_shadow_user_command_permissions() {
        // Verify shadow model matches real for a specific configuration
        let mut shadow = ShadowUser::new_default_off();
        let mut real = AclUser::new("test".to_string());

        // Apply same rules to both
        shadow.apply_rule("+@read");
        apply_rule(&mut real, "+@read").unwrap();

        shadow.apply_rule("-GET");
        apply_rule(&mut real, "-GET").unwrap();

        shadow.apply_rule("+SET");
        apply_rule(&mut real, "+SET").unwrap();

        // Check agreement on a set of commands
        for cmd in TEST_COMMANDS {
            let shadow_ok = shadow.is_command_permitted(cmd);
            let real_ok = real.commands.is_command_permitted(cmd);
            assert_eq!(
                shadow_ok, real_ok,
                "Command '{}': shadow={}, real={}",
                cmd, shadow_ok, real_ok
            );
        }
    }

    #[test]
    fn test_shadow_user_key_permissions() {
        let mut shadow = ShadowUser::new_default_off();
        shadow.key_patterns.push("user:*".to_string());
        shadow.key_patterns.push("cache:*".to_string());

        let mut real_keys = KeyPatterns::deny_all();
        real_keys.add("user:*");
        real_keys.add("cache:*");

        let test_keys = &[
            "user:123",
            "cache:foo",
            "admin:secret",
            "other",
            "user:",
            "cache:",
        ];

        for key in test_keys {
            let shadow_ok = shadow.is_key_permitted(key);
            let real_ok = real_keys.is_key_permitted(key);
            assert_eq!(
                shadow_ok, real_ok,
                "Key '{}': shadow={}, real={}",
                key, shadow_ok, real_ok
            );
        }
    }
}
