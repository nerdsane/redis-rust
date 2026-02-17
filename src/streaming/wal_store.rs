//! WAL Storage Abstraction
//!
//! Provides trait-based abstractions for WAL file operations, following the
//! existing I/O patterns in `object_store.rs` and `src/io/mod.rs`.
//!
//! ## Implementations
//!
//! - `InMemoryWalStore`: For unit tests and DST
//! - `LocalWalStore`: For production (std::fs::File + sync_all)
//! - `SimulatedWalStore`: For DST with buggify fault injection

use std::collections::HashMap;
use std::io::{Error as IoError, ErrorKind, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Error type for WAL operations
#[derive(Debug)]
pub enum WalError {
    /// I/O error
    Io(IoError),
    /// Corruption detected (CRC mismatch)
    Corruption(String),
    /// WAL file not found
    NotFound(String),
    /// Disk full
    DiskFull,
    /// Fsync failed
    FsyncFailed(String),
    /// Partial write detected
    PartialWrite { expected: usize, actual: usize },
}

impl std::fmt::Display for WalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalError::Io(e) => write!(f, "WAL I/O error: {}", e),
            WalError::Corruption(msg) => write!(f, "WAL corruption: {}", msg),
            WalError::NotFound(name) => write!(f, "WAL file not found: {}", name),
            WalError::DiskFull => write!(f, "WAL disk full"),
            WalError::FsyncFailed(msg) => write!(f, "WAL fsync failed: {}", msg),
            WalError::PartialWrite { expected, actual } => {
                write!(
                    f,
                    "WAL partial write: expected {} bytes, wrote {}",
                    expected, actual
                )
            }
        }
    }
}

impl std::error::Error for WalError {}

impl From<IoError> for WalError {
    fn from(e: IoError) -> Self {
        match e.kind() {
            ErrorKind::NotFound => WalError::NotFound(e.to_string()),
            _ if e.to_string().contains("No space left") => WalError::DiskFull,
            _ => WalError::Io(e),
        }
    }
}

/// Trait for WAL file writers
pub trait WalFileWriter: Send {
    /// Append data to the WAL file. Returns the byte offset after the write.
    fn append(&mut self, data: &[u8]) -> Result<u64, WalError>;
    /// Fsync the WAL file to durable storage.
    fn sync(&mut self) -> Result<(), WalError>;
    /// Current size of the WAL file in bytes.
    fn size(&self) -> u64;
}

/// Trait for WAL file readers
pub trait WalFileReader: Send {
    /// Read the entire WAL file contents.
    fn read_all(&mut self) -> Result<Vec<u8>, WalError>;
}

/// Trait for WAL storage backends
pub trait WalStore: Send + Sync + 'static {
    type Writer: WalFileWriter;
    type Reader: WalFileReader;

    /// Create a new WAL file for writing.
    fn create(&self, name: &str) -> Result<Self::Writer, WalError>;
    /// Open an existing WAL file for reading.
    fn open_read(&self, name: &str) -> Result<Self::Reader, WalError>;
    /// List all WAL file names, sorted alphabetically.
    fn list(&self) -> Result<Vec<String>, WalError>;
    /// Delete a WAL file.
    fn delete(&self, name: &str) -> Result<(), WalError>;
    /// Check if a WAL file exists.
    fn exists(&self, name: &str) -> Result<bool, WalError>;
}

// ============================================================================
// InMemoryWalStore - For unit tests and DST
// ============================================================================

/// In-memory WAL file data
#[derive(Debug, Clone, Default)]
struct InMemoryFile {
    data: Vec<u8>,
    /// Position up to which data is "synced" (durable)
    synced_pos: usize,
}

/// In-memory WAL store for unit tests and deterministic simulation
#[derive(Debug, Clone)]
pub struct InMemoryWalStore {
    files: Arc<Mutex<HashMap<String, InMemoryFile>>>,
}

impl InMemoryWalStore {
    pub fn new() -> Self {
        InMemoryWalStore {
            files: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get the raw bytes of a WAL file (for testing/crash simulation)
    pub fn get_file_data(&self, name: &str) -> Option<Vec<u8>> {
        let files = self.files.lock().expect("wal store mutex poisoned");
        files.get(name).map(|f| f.data.clone())
    }

    /// Set the raw bytes of a WAL file (for crash simulation - truncate)
    pub fn set_file_data(&self, name: &str, data: Vec<u8>) {
        let mut files = self.files.lock().expect("wal store mutex poisoned");
        if let Some(file) = files.get_mut(name) {
            file.data = data;
        }
    }

    /// Truncate a WAL file to a given length (for crash simulation)
    pub fn truncate_file(&self, name: &str, len: usize) {
        let mut files = self.files.lock().expect("wal store mutex poisoned");
        if let Some(file) = files.get_mut(name) {
            file.data.truncate(len);
            if file.synced_pos > len {
                file.synced_pos = len;
            }
        }
    }

    /// Simulate a crash: truncate all files to their synced position.
    /// Un-synced data is lost (as it would be in a real crash).
    pub fn simulate_crash(&self) {
        let mut files = self.files.lock().expect("wal store mutex poisoned");
        for file in files.values_mut() {
            file.data.truncate(file.synced_pos);
        }
    }

    /// Get the number of WAL files
    pub fn file_count(&self) -> usize {
        self.files.lock().expect("wal store mutex poisoned").len()
    }
}

impl Default for InMemoryWalStore {
    fn default() -> Self {
        Self::new()
    }
}

/// In-memory WAL file writer
pub struct InMemoryWalWriter {
    name: String,
    files: Arc<Mutex<HashMap<String, InMemoryFile>>>,
    current_size: u64,
}

impl WalFileWriter for InMemoryWalWriter {
    fn append(&mut self, data: &[u8]) -> Result<u64, WalError> {
        debug_assert!(!data.is_empty(), "Precondition: data must not be empty");

        let mut files = self.files.lock().expect("wal store mutex poisoned");
        let file = files
            .get_mut(&self.name)
            .expect("WAL file must exist after create");
        file.data.extend_from_slice(data);
        self.current_size = file.data.len() as u64;

        debug_assert!(
            self.current_size >= data.len() as u64,
            "Postcondition: size must be at least data length"
        );

        Ok(self.current_size)
    }

    fn sync(&mut self) -> Result<(), WalError> {
        // Mark the current position as synced (durable)
        let mut files = self.files.lock().expect("wal store mutex poisoned");
        if let Some(file) = files.get_mut(&self.name) {
            file.synced_pos = file.data.len();
        }
        Ok(())
    }

    fn size(&self) -> u64 {
        self.current_size
    }
}

/// In-memory WAL file reader
pub struct InMemoryWalReader {
    data: Vec<u8>,
}

impl WalFileReader for InMemoryWalReader {
    fn read_all(&mut self) -> Result<Vec<u8>, WalError> {
        Ok(self.data.clone())
    }
}

impl WalStore for InMemoryWalStore {
    type Writer = InMemoryWalWriter;
    type Reader = InMemoryWalReader;

    fn create(&self, name: &str) -> Result<Self::Writer, WalError> {
        debug_assert!(!name.is_empty(), "Precondition: name must not be empty");

        let mut files = self.files.lock().expect("wal store mutex poisoned");
        files.insert(name.to_string(), InMemoryFile::default());

        Ok(InMemoryWalWriter {
            name: name.to_string(),
            files: Arc::clone(&self.files),
            current_size: 0,
        })
    }

    fn open_read(&self, name: &str) -> Result<Self::Reader, WalError> {
        let files = self.files.lock().expect("wal store mutex poisoned");
        let file = files
            .get(name)
            .ok_or_else(|| WalError::NotFound(name.to_string()))?;
        Ok(InMemoryWalReader {
            data: file.data.clone(),
        })
    }

    fn list(&self) -> Result<Vec<String>, WalError> {
        let files = self.files.lock().expect("wal store mutex poisoned");
        let mut names: Vec<String> = files.keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    fn delete(&self, name: &str) -> Result<(), WalError> {
        let mut files = self.files.lock().expect("wal store mutex poisoned");
        files.remove(name);
        Ok(())
    }

    fn exists(&self, name: &str) -> Result<bool, WalError> {
        let files = self.files.lock().expect("wal store mutex poisoned");
        Ok(files.contains_key(name))
    }
}

// ============================================================================
// LocalWalStore - For production
// ============================================================================

/// Local filesystem WAL store using std::fs::File + sync_all()
#[derive(Debug, Clone)]
pub struct LocalWalStore {
    dir: PathBuf,
}

impl LocalWalStore {
    /// Create a new local WAL store. Creates the directory if it doesn't exist.
    pub fn new(dir: PathBuf) -> Result<Self, WalError> {
        std::fs::create_dir_all(&dir)?;
        Ok(LocalWalStore { dir })
    }

    fn file_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// Local filesystem WAL file writer
pub struct LocalWalWriter {
    file: std::fs::File,
    current_size: u64,
}

impl WalFileWriter for LocalWalWriter {
    fn append(&mut self, data: &[u8]) -> Result<u64, WalError> {
        debug_assert!(!data.is_empty(), "Precondition: data must not be empty");

        let written = self.file.write(data).map_err(WalError::Io)?;
        if written != data.len() {
            return Err(WalError::PartialWrite {
                expected: data.len(),
                actual: written,
            });
        }
        self.current_size = self
            .current_size
            .checked_add(written as u64)
            .expect("WAL size overflow is unreachable for files < u64::MAX");

        Ok(self.current_size)
    }

    fn sync(&mut self) -> Result<(), WalError> {
        self.file
            .sync_all()
            .map_err(|e| WalError::FsyncFailed(e.to_string()))
    }

    fn size(&self) -> u64 {
        self.current_size
    }
}

/// Local filesystem WAL file reader
pub struct LocalWalReader {
    path: PathBuf,
}

impl WalFileReader for LocalWalReader {
    fn read_all(&mut self) -> Result<Vec<u8>, WalError> {
        std::fs::read(&self.path).map_err(WalError::Io)
    }
}

impl WalStore for LocalWalStore {
    type Writer = LocalWalWriter;
    type Reader = LocalWalReader;

    fn create(&self, name: &str) -> Result<Self::Writer, WalError> {
        debug_assert!(!name.is_empty(), "Precondition: name must not be empty");

        let path = self.file_path(name);
        let file = std::fs::File::create(&path).map_err(WalError::Io)?;
        Ok(LocalWalWriter {
            file,
            current_size: 0,
        })
    }

    fn open_read(&self, name: &str) -> Result<Self::Reader, WalError> {
        let path = self.file_path(name);
        if !path.exists() {
            return Err(WalError::NotFound(name.to_string()));
        }
        Ok(LocalWalReader { path })
    }

    fn list(&self) -> Result<Vec<String>, WalError> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.dir).map_err(WalError::Io)? {
            let entry = entry.map_err(WalError::Io)?;
            if entry.path().is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    fn delete(&self, name: &str) -> Result<(), WalError> {
        let path = self.file_path(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(WalError::Io(e)),
        }
    }

    fn exists(&self, name: &str) -> Result<bool, WalError> {
        Ok(self.file_path(name).exists())
    }
}

// ============================================================================
// SimulatedWalStore - For DST with buggify fault injection
// ============================================================================

use crate::buggify::faults::disk as disk_faults;
use crate::io::Rng;

/// Configuration for simulated WAL fault injection
#[derive(Debug, Clone)]
pub struct SimulatedWalStoreConfig {
    /// Probability of write failure
    pub write_fail_prob: f64,
    /// Probability of partial write
    pub partial_write_prob: f64,
    /// Probability of fsync failure
    pub fsync_fail_prob: f64,
    /// Probability of read corruption
    pub corruption_prob: f64,
    /// Probability of disk full error
    pub disk_full_prob: f64,
}

impl Default for SimulatedWalStoreConfig {
    fn default() -> Self {
        SimulatedWalStoreConfig {
            write_fail_prob: 0.01,
            partial_write_prob: 0.005,
            fsync_fail_prob: 0.005,
            corruption_prob: 0.001,
            disk_full_prob: 0.001,
        }
    }
}

impl SimulatedWalStoreConfig {
    /// No faults - for baseline testing
    pub fn no_faults() -> Self {
        SimulatedWalStoreConfig {
            write_fail_prob: 0.0,
            partial_write_prob: 0.0,
            fsync_fail_prob: 0.0,
            corruption_prob: 0.0,
            disk_full_prob: 0.0,
        }
    }

    /// High chaos configuration for stress testing
    pub fn high_chaos() -> Self {
        SimulatedWalStoreConfig {
            write_fail_prob: 0.05,
            partial_write_prob: 0.02,
            fsync_fail_prob: 0.02,
            corruption_prob: 0.01,
            disk_full_prob: 0.005,
        }
    }
}

/// Statistics for WAL fault injection
#[derive(Debug, Clone, Default)]
pub struct SimulatedWalStoreStats {
    pub write_attempts: u64,
    pub write_failures: u64,
    pub partial_writes: u64,
    pub sync_attempts: u64,
    pub sync_failures: u64,
    pub read_attempts: u64,
    pub read_corruptions: u64,
    pub disk_full_errors: u64,
}

/// Inner state for simulated WAL store
struct SimulatedWalStoreInner<R: Rng> {
    rng: R,
    stats: SimulatedWalStoreStats,
}

/// Simulated WAL store wrapping InMemoryWalStore with fault injection
pub struct SimulatedWalStore<R: Rng> {
    inner: InMemoryWalStore,
    config: SimulatedWalStoreConfig,
    state: Arc<Mutex<SimulatedWalStoreInner<R>>>,
}

impl<R: Rng> SimulatedWalStore<R> {
    pub fn new(rng: R, config: SimulatedWalStoreConfig) -> Self {
        SimulatedWalStore {
            inner: InMemoryWalStore::new(),
            config,
            state: Arc::new(Mutex::new(SimulatedWalStoreInner {
                rng,
                stats: SimulatedWalStoreStats::default(),
            })),
        }
    }

    /// Get current statistics
    pub fn stats(&self) -> SimulatedWalStoreStats {
        self.state
            .lock()
            .expect("simulated wal store mutex poisoned")
            .stats
            .clone()
    }

    /// Get the underlying in-memory store (for crash simulation)
    pub fn inner_store(&self) -> &InMemoryWalStore {
        &self.inner
    }
}

impl<R: Rng> Clone for SimulatedWalStore<R> {
    fn clone(&self) -> Self {
        SimulatedWalStore {
            inner: self.inner.clone(),
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

/// Simulated WAL file writer with fault injection
pub struct SimulatedWalWriter<R: Rng> {
    inner: InMemoryWalWriter,
    config: SimulatedWalStoreConfig,
    state: Arc<Mutex<SimulatedWalStoreInner<R>>>,
}

impl<R: Rng> WalFileWriter for SimulatedWalWriter<R> {
    fn append(&mut self, data: &[u8]) -> Result<u64, WalError> {
        {
            let mut s = self.state.lock().expect("simulated wal store mutex poisoned");
            s.stats.write_attempts = s.stats.write_attempts.saturating_add(1);

            // Check disk full
            if crate::buggify!(&mut s.rng, disk_faults::DISK_FULL, self.config.disk_full_prob) {
                s.stats.disk_full_errors = s.stats.disk_full_errors.saturating_add(1);
                return Err(WalError::DiskFull);
            }

            // Check write failure
            if crate::buggify!(&mut s.rng, disk_faults::WRITE_FAIL, self.config.write_fail_prob) {
                s.stats.write_failures = s.stats.write_failures.saturating_add(1);
                return Err(WalError::Io(IoError::new(
                    ErrorKind::Other,
                    "simulated write failure",
                )));
            }

            // Check partial write — return error (caller should not ack)
            if data.len() > 1
                && crate::buggify!(
                    &mut s.rng,
                    disk_faults::PARTIAL_WRITE,
                    self.config.partial_write_prob
                )
            {
                s.stats.partial_writes = s.stats.partial_writes.saturating_add(1);
                let partial_len = s.rng.gen_range(1, data.len() as u64) as usize;
                // Write partial data then return error (simulates mid-write crash)
                let _ = self.inner.append(&data[..partial_len]);
                return Err(WalError::PartialWrite {
                    expected: data.len(),
                    actual: partial_len,
                });
            }
        }

        self.inner.append(data)
    }

    fn sync(&mut self) -> Result<(), WalError> {
        let mut s = self.state.lock().expect("simulated wal store mutex poisoned");
        s.stats.sync_attempts = s.stats.sync_attempts.saturating_add(1);

        if crate::buggify!(&mut s.rng, disk_faults::FSYNC_FAIL, self.config.fsync_fail_prob) {
            s.stats.sync_failures = s.stats.sync_failures.saturating_add(1);
            return Err(WalError::FsyncFailed("simulated fsync failure".to_string()));
        }

        drop(s); // Release lock before calling inner sync
        // Delegate to inner writer to track synced position
        self.inner.sync()
    }

    fn size(&self) -> u64 {
        self.inner.size()
    }
}

/// Simulated WAL file reader with fault injection
pub struct SimulatedWalReader<R: Rng> {
    inner: InMemoryWalReader,
    config: SimulatedWalStoreConfig,
    state: Arc<Mutex<SimulatedWalStoreInner<R>>>,
}

impl<R: Rng> WalFileReader for SimulatedWalReader<R> {
    fn read_all(&mut self) -> Result<Vec<u8>, WalError> {
        {
            let mut s = self.state.lock().expect("simulated wal store mutex poisoned");
            s.stats.read_attempts = s.stats.read_attempts.saturating_add(1);

            if crate::buggify!(
                &mut s.rng,
                disk_faults::CORRUPTION,
                self.config.corruption_prob
            ) {
                s.stats.read_corruptions = s.stats.read_corruptions.saturating_add(1);
                let mut data = self.inner.read_all()?;
                if !data.is_empty() {
                    let idx = s.rng.gen_range(0, data.len() as u64) as usize;
                    data[idx] ^= 0xFF;
                }
                return Ok(data);
            }
        }

        self.inner.read_all()
    }
}

impl<R: Rng + 'static> WalStore for SimulatedWalStore<R> {
    type Writer = SimulatedWalWriter<R>;
    type Reader = SimulatedWalReader<R>;

    fn create(&self, name: &str) -> Result<Self::Writer, WalError> {
        let inner_writer = self.inner.create(name)?;
        Ok(SimulatedWalWriter {
            inner: inner_writer,
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        })
    }

    fn open_read(&self, name: &str) -> Result<Self::Reader, WalError> {
        let inner_reader = self.inner.open_read(name)?;
        Ok(SimulatedWalReader {
            inner: inner_reader,
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        })
    }

    fn list(&self) -> Result<Vec<String>, WalError> {
        self.inner.list()
    }

    fn delete(&self, name: &str) -> Result<(), WalError> {
        self.inner.delete(name)
    }

    fn exists(&self, name: &str) -> Result<bool, WalError> {
        self.inner.exists(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inmemory_create_write_read() {
        let store = InMemoryWalStore::new();

        // Create and write
        let mut writer = store.create("wal-00000000.wal").unwrap();
        writer.append(b"hello").unwrap();
        writer.append(b" world").unwrap();
        assert_eq!(writer.size(), 11);

        // Read back
        let mut reader = store.open_read("wal-00000000.wal").unwrap();
        let data = reader.read_all().unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn test_inmemory_list() {
        let store = InMemoryWalStore::new();
        store.create("wal-00000002.wal").unwrap();
        store.create("wal-00000000.wal").unwrap();
        store.create("wal-00000001.wal").unwrap();

        let files = store.list().unwrap();
        assert_eq!(
            files,
            vec![
                "wal-00000000.wal",
                "wal-00000001.wal",
                "wal-00000002.wal"
            ]
        );
    }

    #[test]
    fn test_inmemory_delete() {
        let store = InMemoryWalStore::new();
        store.create("wal-00000000.wal").unwrap();
        assert!(store.exists("wal-00000000.wal").unwrap());

        store.delete("wal-00000000.wal").unwrap();
        assert!(!store.exists("wal-00000000.wal").unwrap());
    }

    #[test]
    fn test_inmemory_not_found() {
        let store = InMemoryWalStore::new();
        let result = store.open_read("nonexistent.wal");
        assert!(matches!(result, Err(WalError::NotFound(_))));
    }

    #[test]
    fn test_inmemory_sync_noop() {
        let store = InMemoryWalStore::new();
        let mut writer = store.create("test.wal").unwrap();
        writer.append(b"data").unwrap();
        // sync should succeed (no-op)
        writer.sync().unwrap();
    }

    #[test]
    fn test_inmemory_truncate_for_crash_sim() {
        let store = InMemoryWalStore::new();
        let mut writer = store.create("test.wal").unwrap();
        writer.append(b"0123456789").unwrap();

        // Simulate crash by truncating
        store.truncate_file("test.wal", 5);

        let mut reader = store.open_read("test.wal").unwrap();
        let data = reader.read_all().unwrap();
        assert_eq!(data, b"01234");
    }

    #[test]
    fn test_local_wal_store() {
        let dir = std::env::temp_dir().join(format!(
            "redis-wal-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before Unix epoch")
                .as_nanos()
        ));

        let store = LocalWalStore::new(dir.clone()).unwrap();

        // Create and write
        let mut writer = store.create("wal-00000000.wal").unwrap();
        writer.append(b"hello").unwrap();
        writer.append(b" world").unwrap();
        writer.sync().unwrap();
        assert_eq!(writer.size(), 11);

        // Read back
        let mut reader = store.open_read("wal-00000000.wal").unwrap();
        let data = reader.read_all().unwrap();
        assert_eq!(data, b"hello world");

        // List
        store.create("wal-00000001.wal").unwrap();
        let files = store.list().unwrap();
        assert_eq!(files.len(), 2);

        // Delete
        store.delete("wal-00000000.wal").unwrap();
        assert!(!store.exists("wal-00000000.wal").unwrap());

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_simulated_store_no_faults() {
        use crate::io::simulation::SimulatedRng;

        let rng = SimulatedRng::new(42);
        let store = SimulatedWalStore::new(rng, SimulatedWalStoreConfig::no_faults());

        let mut writer = store.create("test.wal").unwrap();
        writer.append(b"hello").unwrap();
        writer.sync().unwrap();

        let mut reader = store.open_read("test.wal").unwrap();
        let data = reader.read_all().unwrap();
        assert_eq!(data, b"hello");

        let stats = store.stats();
        assert_eq!(stats.write_attempts, 1);
        assert_eq!(stats.write_failures, 0);
        assert_eq!(stats.sync_attempts, 1);
        assert_eq!(stats.sync_failures, 0);
    }

    #[test]
    fn test_simulated_store_write_failure() {
        use crate::io::simulation::SimulatedRng;

        let rng = SimulatedRng::new(42);
        let store = SimulatedWalStore::new(
            rng,
            SimulatedWalStoreConfig {
                write_fail_prob: 1.0,
                ..SimulatedWalStoreConfig::no_faults()
            },
        );

        let mut writer = store.create("test.wal").unwrap();
        let result = writer.append(b"data");
        assert!(result.is_err());

        let stats = store.stats();
        assert!(stats.write_failures > 0 || stats.disk_full_errors > 0);
    }

    #[test]
    fn test_simulated_store_fsync_failure() {
        use crate::io::simulation::SimulatedRng;

        let rng = SimulatedRng::new(42);
        let store = SimulatedWalStore::new(
            rng,
            SimulatedWalStoreConfig {
                fsync_fail_prob: 1.0,
                ..SimulatedWalStoreConfig::no_faults()
            },
        );

        let mut writer = store.create("test.wal").unwrap();
        writer.append(b"data").unwrap();
        let result = writer.sync();
        assert!(result.is_err());

        let stats = store.stats();
        assert_eq!(stats.sync_failures, 1);
    }

    #[test]
    fn test_simulated_store_deterministic() {
        use crate::io::simulation::SimulatedRng;

        let seed = 12345u64;
        let config = SimulatedWalStoreConfig {
            write_fail_prob: 0.3,
            fsync_fail_prob: 0.2,
            ..SimulatedWalStoreConfig::no_faults()
        };

        // Run same sequence with same seed twice
        let mut results1 = Vec::new();
        let mut results2 = Vec::new();

        for (results, rng_seed) in [(&mut results1, seed), (&mut results2, seed)] {
            let rng = SimulatedRng::new(rng_seed);
            let store = SimulatedWalStore::new(rng, config.clone());
            let mut writer = store.create("test.wal").unwrap();

            for _ in 0..20 {
                results.push(writer.append(b"data").is_ok());
            }
        }

        assert_eq!(
            results1, results2,
            "Deterministic stores should behave identically"
        );
    }
}
