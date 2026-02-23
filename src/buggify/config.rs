//! BUGGIFY Configuration
//!
//! Defines fault probabilities and provides preset configurations for different
//! testing scenarios (calm, moderate, chaos).

use super::faults;
use std::collections::HashMap;

/// Configuration for fault injection probabilities
#[derive(Debug, Clone)]
pub struct FaultConfig {
    /// Whether BUGGIFY is enabled at all
    pub enabled: bool,
    /// Per-fault probabilities (0.0 to 1.0)
    pub probabilities: HashMap<&'static str, f64>,
    /// Global probability multiplier
    pub global_multiplier: f64,
}

impl Default for FaultConfig {
    fn default() -> Self {
        Self::moderate()
    }
}

impl FaultConfig {
    /// Create a new empty config (all faults disabled)
    pub fn new() -> Self {
        FaultConfig {
            enabled: true,
            probabilities: HashMap::new(),
            global_multiplier: 1.0,
        }
    }

    /// Disabled - no fault injection
    pub fn disabled() -> Self {
        FaultConfig {
            enabled: false,
            probabilities: HashMap::new(),
            global_multiplier: 0.0,
        }
    }

    /// Calm - very low fault rates for basic testing
    pub fn calm() -> Self {
        let mut config = Self::new();
        config.global_multiplier = 0.1;

        // Network (very rare)
        config.set(faults::network::PACKET_DROP, 0.001);
        config.set(faults::network::DELAY, 0.01);

        // Timer (minimal)
        config.set(faults::timer::DRIFT_FAST, 0.001);
        config.set(faults::timer::DRIFT_SLOW, 0.001);

        config
    }

    /// Moderate - balanced fault injection for regular testing
    pub fn moderate() -> Self {
        let mut config = Self::new();
        config.global_multiplier = 1.0;

        // Network faults
        config.set(faults::network::PACKET_DROP, 0.01); // 1%
        config.set(faults::network::PACKET_CORRUPT, 0.001); // 0.1%
        config.set(faults::network::PARTIAL_WRITE, 0.005); // 0.5%
        config.set(faults::network::REORDER, 0.02); // 2%
        config.set(faults::network::CONNECTION_RESET, 0.005); // 0.5%
        config.set(faults::network::CONNECT_TIMEOUT, 0.01); // 1%
        config.set(faults::network::DELAY, 0.05); // 5%
        config.set(faults::network::DUPLICATE, 0.005); // 0.5%

        // Timer faults
        config.set(faults::timer::DRIFT_FAST, 0.01); // 1%
        config.set(faults::timer::DRIFT_SLOW, 0.01); // 1%
        config.set(faults::timer::SKIP, 0.01); // 1%
        config.set(faults::timer::DUPLICATE, 0.005); // 0.5%
        config.set(faults::timer::JUMP_FORWARD, 0.001); // 0.1%
        config.set(faults::timer::JUMP_BACKWARD, 0.0005); // 0.05%

        // Process faults
        config.set(faults::process::CRASH, 0.001); // 0.1%
        config.set(faults::process::PAUSE, 0.01); // 1%
        config.set(faults::process::SLOW, 0.02); // 2%
        config.set(faults::process::OOM, 0.0001); // 0.01%
        config.set(faults::process::CPU_STARVATION, 0.01); // 1%

        // Disk faults (for future persistence)
        config.set(faults::disk::WRITE_FAIL, 0.001); // 0.1%
        config.set(faults::disk::PARTIAL_WRITE, 0.001); // 0.1%
        config.set(faults::disk::CORRUPTION, 0.0001); // 0.01%
        config.set(faults::disk::SLOW, 0.02); // 2%
        config.set(faults::disk::FSYNC_FAIL, 0.0005); // 0.05%
        config.set(faults::disk::STALE_READ, 0.001); // 0.1%
        config.set(faults::disk::DISK_FULL, 0.0001); // 0.01%

        // Replication faults
        config.set(faults::replication::GOSSIP_DROP, 0.02); // 2%
        config.set(faults::replication::GOSSIP_DELAY, 0.05); // 5%
        config.set(faults::replication::GOSSIP_CORRUPT, 0.001); // 0.1%
        config.set(faults::replication::SPLIT_BRAIN, 0.0001); // 0.01%
        config.set(faults::replication::STALE_REPLICA, 0.01); // 1%

        config
    }

    /// Chaos - aggressive fault injection for stress testing
    pub fn chaos() -> Self {
        let mut config = Self::new();
        config.global_multiplier = 3.0;

        // Network faults (high)
        config.set(faults::network::PACKET_DROP, 0.05); // 5%
        config.set(faults::network::PACKET_CORRUPT, 0.01); // 1%
        config.set(faults::network::PARTIAL_WRITE, 0.02); // 2%
        config.set(faults::network::REORDER, 0.10); // 10%
        config.set(faults::network::CONNECTION_RESET, 0.02); // 2%
        config.set(faults::network::CONNECT_TIMEOUT, 0.05); // 5%
        config.set(faults::network::DELAY, 0.15); // 15%
        config.set(faults::network::DUPLICATE, 0.02); // 2%

        // Timer faults (high)
        config.set(faults::timer::DRIFT_FAST, 0.05); // 5%
        config.set(faults::timer::DRIFT_SLOW, 0.05); // 5%
        config.set(faults::timer::SKIP, 0.05); // 5%
        config.set(faults::timer::DUPLICATE, 0.02); // 2%
        config.set(faults::timer::JUMP_FORWARD, 0.01); // 1%
        config.set(faults::timer::JUMP_BACKWARD, 0.005); // 0.5%

        // Process faults (elevated)
        config.set(faults::process::CRASH, 0.005); // 0.5%
        config.set(faults::process::PAUSE, 0.05); // 5%
        config.set(faults::process::SLOW, 0.10); // 10%
        config.set(faults::process::OOM, 0.001); // 0.1%
        config.set(faults::process::CPU_STARVATION, 0.05); // 5%

        // Disk faults (elevated)
        config.set(faults::disk::WRITE_FAIL, 0.005); // 0.5%
        config.set(faults::disk::PARTIAL_WRITE, 0.005); // 0.5%
        config.set(faults::disk::CORRUPTION, 0.001); // 0.1%
        config.set(faults::disk::SLOW, 0.10); // 10%
        config.set(faults::disk::FSYNC_FAIL, 0.002); // 0.2%
        config.set(faults::disk::STALE_READ, 0.005); // 0.5%
        config.set(faults::disk::DISK_FULL, 0.001); // 0.1%

        // Replication faults (high)
        config.set(faults::replication::GOSSIP_DROP, 0.10); // 10%
        config.set(faults::replication::GOSSIP_DELAY, 0.15); // 15%
        config.set(faults::replication::GOSSIP_CORRUPT, 0.005); // 0.5%
        config.set(faults::replication::SPLIT_BRAIN, 0.001); // 0.1%
        config.set(faults::replication::STALE_REPLICA, 0.05); // 5%

        config
    }

    /// Set probability for a specific fault
    pub fn set(&mut self, fault_id: &'static str, probability: f64) -> &mut Self {
        self.probabilities
            .insert(fault_id, probability.clamp(0.0, 1.0));
        self
    }

    /// Get probability for a fault (returns 0.0 if not set)
    pub fn get(&self, fault_id: &str) -> f64 {
        if !self.enabled {
            return 0.0;
        }
        let base = self.probabilities.get(fault_id).copied().unwrap_or(0.0);
        (base * self.global_multiplier).clamp(0.0, 1.0)
    }

    /// Check if a fault should trigger given its probability
    pub fn should_trigger(&self, fault_id: &str, random_value: f64) -> bool {
        random_value < self.get(fault_id)
    }

    /// Builder pattern - enable specific fault category
    pub fn with_network_faults(mut self) -> Self {
        self.set(faults::network::PACKET_DROP, 0.01);
        self.set(faults::network::PACKET_CORRUPT, 0.001);
        self.set(faults::network::REORDER, 0.02);
        self.set(faults::network::DELAY, 0.05);
        self
    }

    /// Builder pattern - enable timer faults
    pub fn with_timer_faults(mut self) -> Self {
        self.set(faults::timer::DRIFT_FAST, 0.01);
        self.set(faults::timer::DRIFT_SLOW, 0.01);
        self.set(faults::timer::SKIP, 0.01);
        self
    }

    /// Builder pattern - enable process faults
    pub fn with_process_faults(mut self) -> Self {
        self.set(faults::process::CRASH, 0.001);
        self.set(faults::process::PAUSE, 0.01);
        self.set(faults::process::SLOW, 0.02);
        self
    }

    /// Builder pattern - set global multiplier
    pub fn with_multiplier(mut self, multiplier: f64) -> Self {
        self.global_multiplier = multiplier.max(0.0);
        self
    }

    /// Load from BUGGIFY_CONFIG env var, or fall back to moderate().
    /// Format: "global_multiplier=1.5,network.packet_drop=0.05,..."
    pub fn from_env_or_default() -> Self {
        match std::env::var("BUGGIFY_CONFIG") {
            Ok(s) if !s.is_empty() => {
                Self::parse_config_string(&s).unwrap_or_else(|e| {
                    eprintln!("BUGGIFY_CONFIG parse error: {e}. Using moderate().");
                    Self::moderate()
                })
            }
            _ => Self::moderate(),
        }
    }

    /// Parse a config string in "key=value,key=value" format.
    pub fn parse_config_string(s: &str) -> Result<Self, String> {
        let mut config = Self::new();
        for pair in s.split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (key, val) = pair
                .split_once('=')
                .ok_or_else(|| format!("invalid pair: {pair}"))?;
            let val: f64 = val
                .trim()
                .parse()
                .map_err(|e| format!("invalid value for {key}: {e}"))?;
            let key = key.trim();
            if key == "global_multiplier" {
                config.global_multiplier = val.max(0.0);
            } else if let Some(fault_id) = fault_key_to_static(key) {
                config.set(fault_id, val);
            }
            // Unknown keys are silently ignored (forward compatibility)
        }
        Ok(config)
    }
}

/// Map dotted fault key string to the corresponding static fault constant.
fn fault_key_to_static(key: &str) -> Option<&'static str> {
    match key {
        // Network
        "network.packet_drop" => Some(faults::network::PACKET_DROP),
        "network.packet_corrupt" => Some(faults::network::PACKET_CORRUPT),
        "network.partial_write" => Some(faults::network::PARTIAL_WRITE),
        "network.reorder" => Some(faults::network::REORDER),
        "network.connection_reset" => Some(faults::network::CONNECTION_RESET),
        "network.connect_timeout" => Some(faults::network::CONNECT_TIMEOUT),
        "network.delay" => Some(faults::network::DELAY),
        "network.duplicate" => Some(faults::network::DUPLICATE),
        // Timer
        "timer.drift_fast" => Some(faults::timer::DRIFT_FAST),
        "timer.drift_slow" => Some(faults::timer::DRIFT_SLOW),
        "timer.skip" => Some(faults::timer::SKIP),
        "timer.duplicate" => Some(faults::timer::DUPLICATE),
        "timer.jump_forward" => Some(faults::timer::JUMP_FORWARD),
        "timer.jump_backward" => Some(faults::timer::JUMP_BACKWARD),
        // Process
        "process.crash" => Some(faults::process::CRASH),
        "process.pause" => Some(faults::process::PAUSE),
        "process.slow" => Some(faults::process::SLOW),
        "process.oom" => Some(faults::process::OOM),
        "process.cpu_starvation" => Some(faults::process::CPU_STARVATION),
        // Disk
        "disk.write_fail" => Some(faults::disk::WRITE_FAIL),
        "disk.partial_write" => Some(faults::disk::PARTIAL_WRITE),
        "disk.corruption" => Some(faults::disk::CORRUPTION),
        "disk.slow" => Some(faults::disk::SLOW),
        "disk.fsync_fail" => Some(faults::disk::FSYNC_FAIL),
        "disk.stale_read" => Some(faults::disk::STALE_READ),
        "disk.disk_full" => Some(faults::disk::DISK_FULL),
        // Object Store
        "object_store.put_fail" => Some(faults::object_store::PUT_FAIL),
        "object_store.get_fail" => Some(faults::object_store::GET_FAIL),
        "object_store.get_corrupt" => Some(faults::object_store::GET_CORRUPT),
        "object_store.timeout" => Some(faults::object_store::TIMEOUT),
        "object_store.partial_write" => Some(faults::object_store::PARTIAL_WRITE),
        "object_store.delete_fail" => Some(faults::object_store::DELETE_FAIL),
        "object_store.list_incomplete" => Some(faults::object_store::LIST_INCOMPLETE),
        "object_store.rename_fail" => Some(faults::object_store::RENAME_FAIL),
        "object_store.slow" => Some(faults::object_store::SLOW),
        // Replication
        "replication.gossip_drop" => Some(faults::replication::GOSSIP_DROP),
        "replication.gossip_delay" => Some(faults::replication::GOSSIP_DELAY),
        "replication.gossip_corrupt" => Some(faults::replication::GOSSIP_CORRUPT),
        "replication.split_brain" => Some(faults::replication::SPLIT_BRAIN),
        "replication.stale_replica" => Some(faults::replication::STALE_REPLICA),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_config() {
        let config = FaultConfig::disabled();
        assert_eq!(config.get(faults::network::PACKET_DROP), 0.0);
        assert!(!config.should_trigger(faults::network::PACKET_DROP, 0.0));
    }

    #[test]
    fn test_moderate_config() {
        let config = FaultConfig::moderate();
        assert!(config.get(faults::network::PACKET_DROP) > 0.0);
        assert!(config.get(faults::network::PACKET_DROP) <= 1.0);
    }

    #[test]
    fn test_chaos_higher_than_moderate() {
        let moderate = FaultConfig::moderate();
        let chaos = FaultConfig::chaos();

        assert!(
            chaos.get(faults::network::PACKET_DROP) > moderate.get(faults::network::PACKET_DROP)
        );
    }

    #[test]
    fn test_should_trigger() {
        let config = FaultConfig::moderate();
        let prob = config.get(faults::network::PACKET_DROP);

        // Value below probability should trigger
        assert!(config.should_trigger(faults::network::PACKET_DROP, prob - 0.001));
        // Value above probability should not trigger
        assert!(!config.should_trigger(faults::network::PACKET_DROP, prob + 0.001));
    }

    #[test]
    fn test_builder_pattern() {
        let config = FaultConfig::new()
            .with_network_faults()
            .with_multiplier(2.0);

        assert!(config.get(faults::network::PACKET_DROP) > 0.0);
        assert_eq!(config.global_multiplier, 2.0);
    }

    #[test]
    fn test_parse_config_string_basic() {
        let config = FaultConfig::parse_config_string(
            "global_multiplier=2.0,network.packet_drop=0.05"
        ).unwrap();
        assert_eq!(config.global_multiplier, 2.0);
        assert!((config.probabilities[faults::network::PACKET_DROP] - 0.05).abs() < 1e-9);
    }

    #[test]
    fn test_parse_config_string_empty() {
        let config = FaultConfig::parse_config_string("").unwrap();
        assert_eq!(config.global_multiplier, 1.0); // default from new()
    }

    #[test]
    fn test_parse_config_string_whitespace() {
        let config = FaultConfig::parse_config_string(
            " global_multiplier = 1.5 , network.delay = 0.1 "
        ).unwrap();
        assert_eq!(config.global_multiplier, 1.5);
        assert!((config.probabilities[faults::network::DELAY] - 0.1).abs() < 1e-9);
    }

    #[test]
    fn test_parse_config_string_all_faults() {
        // Build a config string with all known faults
        let pairs: Vec<String> = faults::ALL_FAULTS.iter()
            .map(|f| format!("{}=0.123", f))
            .collect();
        let s = pairs.join(",");
        let config = FaultConfig::parse_config_string(&s).unwrap();
        for fault in faults::ALL_FAULTS {
            assert!(
                (config.probabilities[*fault] - 0.123).abs() < 1e-9,
                "Fault {fault} not parsed correctly"
            );
        }
    }

    #[test]
    fn test_parse_config_string_invalid_pair() {
        let result = FaultConfig::parse_config_string("no_equals_sign");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_config_string_invalid_value() {
        let result = FaultConfig::parse_config_string("global_multiplier=notanumber");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_config_string_unknown_key_ignored() {
        let config = FaultConfig::parse_config_string(
            "global_multiplier=1.0,unknown.key=0.5"
        ).unwrap();
        assert_eq!(config.global_multiplier, 1.0);
        // unknown key should be silently ignored
    }

    #[test]
    fn test_from_env_or_default_no_env() {
        // When env var is not set, should return moderate()
        std::env::remove_var("BUGGIFY_CONFIG");
        let config = FaultConfig::from_env_or_default();
        let moderate = FaultConfig::moderate();
        assert_eq!(config.global_multiplier, moderate.global_multiplier);
    }

    #[test]
    fn test_fault_key_to_static_coverage() {
        // Every fault in ALL_FAULTS should be mappable from its string value
        for fault in faults::ALL_FAULTS {
            let result = fault_key_to_static(fault);
            assert!(
                result.is_some(),
                "fault_key_to_static missing mapping for: {fault}"
            );
            assert_eq!(result.unwrap(), *fault);
        }
    }
}
