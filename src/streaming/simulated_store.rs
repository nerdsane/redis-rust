//! Simulated Object Store with Fault Injection
//!
//! DST-compatible wrapper that injects faults using buggify.
//! Follows FoundationDB patterns for deterministic simulation testing.

use crate::buggify::faults::object_store as faults;
use crate::io::Rng;
use crate::streaming::{ListResult, ObjectMeta, ObjectStore};
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// Configuration for simulated fault injection
#[derive(Debug, Clone)]
pub struct SimulatedStoreConfig {
    /// Probability of PUT operation failure
    pub put_fail_prob: f64,
    /// Probability of GET operation failure
    pub get_fail_prob: f64,
    /// Probability of GET returning corrupted data
    pub get_corrupt_prob: f64,
    /// Probability of operation timeout
    pub timeout_prob: f64,
    /// Probability of partial write
    pub partial_write_prob: f64,
    /// Probability of DELETE failure
    pub delete_fail_prob: f64,
    /// Probability of LIST returning incomplete results
    pub list_incomplete_prob: f64,
    /// Probability of RENAME failure
    pub rename_fail_prob: f64,
    /// Simulated latency range in microseconds (min, max)
    pub latency_range_us: (u64, u64),
}

impl Default for SimulatedStoreConfig {
    fn default() -> Self {
        SimulatedStoreConfig {
            put_fail_prob: 0.01,             // 1%
            get_fail_prob: 0.01,             // 1%
            get_corrupt_prob: 0.001,         // 0.1%
            timeout_prob: 0.005,             // 0.5%
            partial_write_prob: 0.005,       // 0.5%
            delete_fail_prob: 0.01,          // 1%
            list_incomplete_prob: 0.02,      // 2%
            rename_fail_prob: 0.01,          // 1%
            latency_range_us: (100, 10_000), // 0.1ms - 10ms
        }
    }
}

impl SimulatedStoreConfig {
    /// High chaos configuration for stress testing
    pub fn high_chaos() -> Self {
        SimulatedStoreConfig {
            put_fail_prob: 0.05,
            get_fail_prob: 0.05,
            get_corrupt_prob: 0.01,
            timeout_prob: 0.02,
            partial_write_prob: 0.02,
            delete_fail_prob: 0.05,
            list_incomplete_prob: 0.05,
            rename_fail_prob: 0.05,
            latency_range_us: (1_000, 100_000),
        }
    }

    /// No faults - for baseline testing
    pub fn no_faults() -> Self {
        SimulatedStoreConfig {
            put_fail_prob: 0.0,
            get_fail_prob: 0.0,
            get_corrupt_prob: 0.0,
            timeout_prob: 0.0,
            partial_write_prob: 0.0,
            delete_fail_prob: 0.0,
            list_incomplete_prob: 0.0,
            rename_fail_prob: 0.0,
            latency_range_us: (0, 0),
        }
    }
}

/// Statistics for fault injection
#[derive(Debug, Clone, Default)]
pub struct SimulatedStoreStats {
    pub put_attempts: u64,
    pub put_failures: u64,
    pub get_attempts: u64,
    pub get_failures: u64,
    pub get_corruptions: u64,
    pub delete_attempts: u64,
    pub delete_failures: u64,
    pub list_attempts: u64,
    pub list_incomplete: u64,
    pub rename_attempts: u64,
    pub rename_failures: u64,
    pub timeouts: u64,
    pub partial_writes: u64,
}

/// Inner state for the simulated store
struct SimulatedStoreInner<R: Rng> {
    rng: R,
    stats: SimulatedStoreStats,
}

/// Simulated object store that wraps another store and injects faults
pub struct SimulatedObjectStore<S: ObjectStore + Clone, R: Rng> {
    inner_store: S,
    config: SimulatedStoreConfig,
    state: Arc<Mutex<SimulatedStoreInner<R>>>,
}

impl<S: ObjectStore + Clone, R: Rng> SimulatedObjectStore<S, R> {
    /// Create a new simulated store with the given RNG
    pub fn new(inner_store: S, rng: R, config: SimulatedStoreConfig) -> Self {
        SimulatedObjectStore {
            inner_store,
            config,
            state: Arc::new(Mutex::new(SimulatedStoreInner {
                rng,
                stats: SimulatedStoreStats::default(),
            })),
        }
    }

    /// Get current statistics
    pub fn stats(&self) -> SimulatedStoreStats {
        self.state
            .lock()
            .expect("simulated store mutex poisoned")
            .stats
            .clone()
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        self.state
            .lock()
            .expect("simulated store mutex poisoned")
            .stats = SimulatedStoreStats::default();
    }
}

impl<S: ObjectStore + Clone + 'static, R: Rng + 'static> ObjectStore
    for SimulatedObjectStore<S, R>
{
    fn put(&self, key: &str, data: &[u8]) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send>> {
        let key = key.to_string();
        let data = data.to_vec();
        let inner = self.inner_store.clone();
        let state = self.state.clone();
        let config = self.config.clone();

        Box::pin(async move {
            {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                s.stats.put_attempts += 1;
            }

            // Check for timeout
            let should_timeout = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::TIMEOUT, config.timeout_prob)
            };
            if should_timeout {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .timeouts += 1;
                return Err(IoError::new(ErrorKind::TimedOut, "simulated timeout"));
            }

            // Check for put failure
            let should_fail = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::PUT_FAIL, config.put_fail_prob)
            };
            if should_fail {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .put_failures += 1;
                return Err(IoError::new(ErrorKind::Other, "simulated put failure"));
            }

            // Check for partial write
            let should_partial = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::PARTIAL_WRITE, config.partial_write_prob)
            };
            let write_data = if should_partial && data.len() > 1 {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .partial_writes += 1;
                let new_len = {
                    let mut s = state.lock().expect("simulated store mutex poisoned");
                    s.rng.gen_range(1, data.len() as u64) as usize
                };
                data[..new_len].to_vec()
            } else {
                data
            };

            // Simulate latency
            let (min, max) = config.latency_range_us;
            if min > 0 || max > 0 {
                let latency_us = {
                    let mut s = state.lock().expect("simulated store mutex poisoned");
                    if max > min {
                        s.rng.gen_range(min, max)
                    } else {
                        min
                    }
                };
                if latency_us > 0 {
                    tokio::time::sleep(std::time::Duration::from_micros(latency_us)).await;
                }
            }

            inner.put(&key, &write_data).await
        })
    }

    fn get(&self, key: &str) -> Pin<Box<dyn Future<Output = IoResult<Vec<u8>>> + Send>> {
        let key = key.to_string();
        let inner = self.inner_store.clone();
        let state = self.state.clone();
        let config = self.config.clone();

        Box::pin(async move {
            {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                s.stats.get_attempts += 1;
            }

            // Check for timeout
            let should_timeout = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::TIMEOUT, config.timeout_prob)
            };
            if should_timeout {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .timeouts += 1;
                return Err(IoError::new(ErrorKind::TimedOut, "simulated timeout"));
            }

            // Check for get failure
            let should_fail = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::GET_FAIL, config.get_fail_prob)
            };
            if should_fail {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .get_failures += 1;
                return Err(IoError::new(ErrorKind::Other, "simulated get failure"));
            }

            // Simulate latency
            let (min, max) = config.latency_range_us;
            if min > 0 || max > 0 {
                let latency_us = {
                    let mut s = state.lock().expect("simulated store mutex poisoned");
                    if max > min {
                        s.rng.gen_range(min, max)
                    } else {
                        min
                    }
                };
                if latency_us > 0 {
                    tokio::time::sleep(std::time::Duration::from_micros(latency_us)).await;
                }
            }

            let data = inner.get(&key).await?;

            // Check for corruption
            let should_corrupt = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::GET_CORRUPT, config.get_corrupt_prob)
            };
            if should_corrupt && !data.is_empty() {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .get_corruptions += 1;
                let mut corrupted = data;
                let idx = {
                    let mut s = state.lock().expect("simulated store mutex poisoned");
                    s.rng.gen_range(0, corrupted.len() as u64) as usize
                };
                corrupted[idx] ^= 0xFF;
                return Ok(corrupted);
            }

            Ok(data)
        })
    }

    fn exists(&self, key: &str) -> Pin<Box<dyn Future<Output = IoResult<bool>> + Send>> {
        let key = key.to_string();
        let inner = self.inner_store.clone();
        Box::pin(async move { inner.exists(&key).await })
    }

    fn delete(&self, key: &str) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send>> {
        let key = key.to_string();
        let inner = self.inner_store.clone();
        let state = self.state.clone();
        let config = self.config.clone();

        Box::pin(async move {
            {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                s.stats.delete_attempts += 1;
            }

            // Check for delete failure
            let should_fail = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::DELETE_FAIL, config.delete_fail_prob)
            };
            if should_fail {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .delete_failures += 1;
                return Err(IoError::new(ErrorKind::Other, "simulated delete failure"));
            }

            inner.delete(&key).await
        })
    }

    fn list(
        &self,
        prefix: &str,
        continuation_token: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = IoResult<ListResult>> + Send>> {
        let prefix = prefix.to_string();
        let token = continuation_token.map(|s| s.to_string());
        let inner = self.inner_store.clone();
        let state = self.state.clone();
        let config = self.config.clone();

        Box::pin(async move {
            {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                s.stats.list_attempts += 1;
            }

            let result = inner.list(&prefix, token.as_deref()).await?;

            // Check for incomplete listing
            let should_truncate = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(
                    &mut s.rng,
                    faults::LIST_INCOMPLETE,
                    config.list_incomplete_prob
                )
            };
            if should_truncate && result.objects.len() > 1 {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .list_incomplete += 1;
                let truncate_at = {
                    let mut s = state.lock().expect("simulated store mutex poisoned");
                    s.rng.gen_range(1, result.objects.len() as u64) as usize
                };
                return Ok(ListResult {
                    objects: result.objects.into_iter().take(truncate_at).collect(),
                    continuation_token: Some("truncated".to_string()),
                });
            }

            Ok(result)
        })
    }

    fn rename(&self, from: &str, to: &str) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send>> {
        let from = from.to_string();
        let to = to.to_string();
        let inner = self.inner_store.clone();
        let state = self.state.clone();
        let config = self.config.clone();

        Box::pin(async move {
            {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                s.stats.rename_attempts += 1;
            }

            // Check for rename failure
            let should_fail = {
                let mut s = state.lock().expect("simulated store mutex poisoned");
                crate::buggify!(&mut s.rng, faults::RENAME_FAIL, config.rename_fail_prob)
            };
            if should_fail {
                state
                    .lock()
                    .expect("simulated store mutex poisoned")
                    .stats
                    .rename_failures += 1;
                return Err(IoError::new(ErrorKind::Other, "simulated rename failure"));
            }

            inner.rename(&from, &to).await
        })
    }

    fn head(&self, key: &str) -> Pin<Box<dyn Future<Output = IoResult<ObjectMeta>> + Send>> {
        let key = key.to_string();
        let inner = self.inner_store.clone();
        Box::pin(async move { inner.head(&key).await })
    }
}

// Implement Clone for SimulatedObjectStore
impl<S: ObjectStore + Clone, R: Rng> Clone for SimulatedObjectStore<S, R> {
    fn clone(&self) -> Self {
        SimulatedObjectStore {
            inner_store: self.inner_store.clone(),
            config: self.config.clone(),
            state: self.state.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::simulation::SimulatedRng;
    use crate::streaming::InMemoryObjectStore;

    #[tokio::test]
    async fn test_simulated_store_no_faults() {
        let inner = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(42);
        let store = SimulatedObjectStore::new(inner, rng, SimulatedStoreConfig::no_faults());

        // Should work without faults
        store.put("key1", b"value1").await.unwrap();
        let data = store.get("key1").await.unwrap();
        assert_eq!(data, b"value1");

        let stats = store.stats();
        assert_eq!(stats.put_attempts, 1);
        assert_eq!(stats.put_failures, 0);
        assert_eq!(stats.get_attempts, 1);
        assert_eq!(stats.get_failures, 0);
    }

    #[tokio::test]
    async fn test_simulated_store_deterministic() {
        // Two stores with same seed should behave identically
        let seed = 12345u64;

        let inner1 = InMemoryObjectStore::new();
        let rng1 = SimulatedRng::new(seed);
        let store1 = SimulatedObjectStore::new(
            inner1,
            rng1,
            SimulatedStoreConfig {
                put_fail_prob: 0.5,
                ..SimulatedStoreConfig::no_faults()
            },
        );

        let inner2 = InMemoryObjectStore::new();
        let rng2 = SimulatedRng::new(seed);
        let store2 = SimulatedObjectStore::new(
            inner2,
            rng2,
            SimulatedStoreConfig {
                put_fail_prob: 0.5,
                ..SimulatedStoreConfig::no_faults()
            },
        );

        // Run same operations on both
        let mut results1 = Vec::new();
        let mut results2 = Vec::new();

        for i in 0..20 {
            results1.push(store1.put(&format!("key{}", i), b"data").await.is_ok());
            results2.push(store2.put(&format!("key{}", i), b"data").await.is_ok());
        }

        // Results should be identical
        assert_eq!(
            results1, results2,
            "Deterministic stores should behave identically"
        );
    }

    #[tokio::test]
    async fn test_simulated_store_fault_injection() {
        let inner = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(999);
        let store = SimulatedObjectStore::new(
            inner,
            rng,
            SimulatedStoreConfig {
                put_fail_prob: 1.0, // Always fail
                ..SimulatedStoreConfig::no_faults()
            },
        );

        // Should always fail
        let result = store.put("key", b"value").await;
        assert!(result.is_err());

        let stats = store.stats();
        assert_eq!(stats.put_failures, 1);
    }

    #[tokio::test]
    async fn test_simulated_store_corruption() {
        let inner = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(42);
        let store = SimulatedObjectStore::new(
            inner,
            rng,
            SimulatedStoreConfig {
                get_corrupt_prob: 1.0, // Always corrupt
                ..SimulatedStoreConfig::no_faults()
            },
        );

        store.put("key", b"original data here").await.unwrap();
        let data = store.get("key").await.unwrap();

        // Data should be corrupted (different from original)
        assert_ne!(data, b"original data here");

        let stats = store.stats();
        assert_eq!(stats.get_corruptions, 1);
    }

    #[tokio::test]
    async fn test_simulated_store_high_chaos() {
        let inner = InMemoryObjectStore::new();
        let rng = SimulatedRng::new(42);
        let store = SimulatedObjectStore::new(inner, rng, SimulatedStoreConfig::high_chaos());

        // Run many operations - some should fail
        let mut successes = 0;
        let mut failures = 0;

        for i in 0..100 {
            match store.put(&format!("key{}", i), b"data").await {
                Ok(_) => successes += 1,
                Err(_) => failures += 1,
            }
        }

        // With high chaos, we expect some failures
        assert!(
            failures > 0,
            "Expected some failures with high chaos config"
        );
        assert!(
            successes > 0,
            "Expected some successes even with high chaos"
        );

        let stats = store.stats();
        assert!(stats.put_failures > 0 || stats.timeouts > 0 || stats.partial_writes > 0);
    }
}
