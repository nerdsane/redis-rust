//! Object Store Abstraction
//!
//! Provides a trait-based abstraction for object storage operations,
//! following the existing I/O patterns in `src/io/mod.rs`.
//!
//! Implementations:
//! - `InMemoryObjectStore`: For unit tests and DST
//! - `LocalFsObjectStore`: For development and local testing
//! - `S3ObjectStore`: For production (feature-gated)

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

/// Metadata for a stored object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    /// Object key (path)
    pub key: String,
    /// Size in bytes
    pub size_bytes: u64,
    /// Creation timestamp (Unix ms)
    pub created_at_ms: u64,
    /// ETag or content hash (optional)
    pub etag: Option<String>,
}

/// Result of a list operation
#[derive(Debug, Clone, Default)]
pub struct ListResult {
    /// Objects matching the prefix
    pub objects: Vec<ObjectMeta>,
    /// Continuation token for pagination (if more results exist)
    pub continuation_token: Option<String>,
}

/// Error type for object store operations
#[derive(Debug)]
pub enum ObjectStoreError {
    /// Object not found
    NotFound(String),
    /// I/O error
    Io(IoError),
    /// Object already exists (for conditional puts)
    AlreadyExists(String),
    /// Permission denied
    PermissionDenied(String),
    /// Other errors
    Other(String),
}

impl std::fmt::Display for ObjectStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObjectStoreError::NotFound(key) => write!(f, "Object not found: {}", key),
            ObjectStoreError::Io(e) => write!(f, "I/O error: {}", e),
            ObjectStoreError::AlreadyExists(key) => write!(f, "Object already exists: {}", key),
            ObjectStoreError::PermissionDenied(msg) => write!(f, "Permission denied: {}", msg),
            ObjectStoreError::Other(msg) => write!(f, "Object store error: {}", msg),
        }
    }
}

impl std::error::Error for ObjectStoreError {}

impl From<IoError> for ObjectStoreError {
    fn from(e: IoError) -> Self {
        match e.kind() {
            ErrorKind::NotFound => ObjectStoreError::NotFound(e.to_string()),
            ErrorKind::PermissionDenied => ObjectStoreError::PermissionDenied(e.to_string()),
            ErrorKind::AlreadyExists => ObjectStoreError::AlreadyExists(e.to_string()),
            _ => ObjectStoreError::Io(e),
        }
    }
}

/// Object store abstraction trait
///
/// Follows the pattern from `src/io/mod.rs` (Network trait) for
/// consistency and DST compatibility.
pub trait ObjectStore: Send + Sync + 'static {
    /// Put an object (create or overwrite)
    fn put<'a>(
        &'a self,
        key: &'a str,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>>;

    /// Get an object's contents
    fn get<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<Vec<u8>>> + Send + 'a>>;

    /// Check if an object exists
    fn exists<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<bool>> + Send + 'a>>;

    /// Delete an object
    fn delete<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>>;

    /// List objects with a prefix
    fn list<'a>(
        &'a self,
        prefix: &'a str,
        continuation_token: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = IoResult<ListResult>> + Send + 'a>>;

    /// Rename/move an object (for atomic manifest updates)
    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>>;

    /// Get object metadata without downloading content
    fn head<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<ObjectMeta>> + Send + 'a>>;
}

// ============================================================================
// InMemoryObjectStore - For tests and DST
// ============================================================================

/// In-memory object store for unit tests and deterministic simulation
#[derive(Debug)]
pub struct InMemoryObjectStore {
    data: Arc<RwLock<HashMap<String, StoredObject>>>,
}

#[derive(Debug, Clone)]
struct StoredObject {
    data: Vec<u8>,
    created_at_ms: u64,
}

impl InMemoryObjectStore {
    /// Create a new in-memory object store
    pub fn new() -> Self {
        InMemoryObjectStore {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get current timestamp (for testing, uses system time)
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before Unix epoch")
            .as_millis() as u64
    }

    /// Get the number of stored objects (for testing)
    pub fn len(&self) -> usize {
        self.data.read().len()
    }

    /// Check if empty (for testing)
    pub fn is_empty(&self) -> bool {
        self.data.read().is_empty()
    }

    /// Clear all objects (for testing)
    pub fn clear(&self) {
        self.data.write().clear();
    }
}

impl Default for InMemoryObjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for InMemoryObjectStore {
    fn clone(&self) -> Self {
        InMemoryObjectStore {
            data: Arc::clone(&self.data),
        }
    }
}

impl ObjectStore for InMemoryObjectStore {
    fn put<'a>(
        &'a self,
        key: &'a str,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let obj = StoredObject {
                data: data.to_vec(),
                created_at_ms: Self::now_ms(),
            };
            self.data.write().insert(key.to_string(), obj);
            Ok(())
        })
    }

    fn get<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            self.data
                .read()
                .get(key)
                .map(|obj| obj.data.clone())
                .ok_or_else(|| IoError::new(ErrorKind::NotFound, format!("Key not found: {}", key)))
        })
    }

    fn exists<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<bool>> + Send + 'a>> {
        Box::pin(async move { Ok(self.data.read().contains_key(key)) })
    }

    fn delete<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            self.data.write().remove(key);
            Ok(())
        })
    }

    fn list<'a>(
        &'a self,
        prefix: &'a str,
        _continuation_token: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = IoResult<ListResult>> + Send + 'a>> {
        Box::pin(async move {
            let data = self.data.read();
            let mut objects: Vec<ObjectMeta> = data
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(k, v)| ObjectMeta {
                    key: k.clone(),
                    size_bytes: v.data.len() as u64,
                    created_at_ms: v.created_at_ms,
                    etag: None,
                })
                .collect();

            // Sort by key for consistent ordering
            objects.sort_by(|a, b| a.key.cmp(&b.key));

            Ok(ListResult {
                objects,
                continuation_token: None,
            })
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut data = self.data.write();
            if let Some(obj) = data.remove(from) {
                data.insert(to.to_string(), obj);
                Ok(())
            } else {
                Err(IoError::new(
                    ErrorKind::NotFound,
                    format!("Source key not found: {}", from),
                ))
            }
        })
    }

    fn head<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<ObjectMeta>> + Send + 'a>> {
        Box::pin(async move {
            self.data
                .read()
                .get(key)
                .map(|obj| ObjectMeta {
                    key: key.to_string(),
                    size_bytes: obj.data.len() as u64,
                    created_at_ms: obj.created_at_ms,
                    etag: None,
                })
                .ok_or_else(|| IoError::new(ErrorKind::NotFound, format!("Key not found: {}", key)))
        })
    }
}

// ============================================================================
// LocalFsObjectStore - For development
// ============================================================================

/// Local filesystem object store for development and testing
#[derive(Debug, Clone)]
pub struct LocalFsObjectStore {
    base_path: PathBuf,
}

impl LocalFsObjectStore {
    /// Create a new local filesystem object store
    pub fn new(base_path: PathBuf) -> Self {
        LocalFsObjectStore { base_path }
    }

    /// Create with a temporary directory (for tests)
    pub fn temp() -> IoResult<Self> {
        let temp_dir = std::env::temp_dir().join(format!(
            "redis-stream-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir)?;
        Ok(LocalFsObjectStore::new(temp_dir))
    }

    /// Get the full path for a key
    fn full_path(&self, key: &str) -> PathBuf {
        self.base_path.join(key)
    }

    /// Ensure parent directories exist
    fn ensure_parent(&self, path: &PathBuf) -> IoResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Get the base path (for testing)
    pub fn base_path(&self) -> &PathBuf {
        &self.base_path
    }
}

impl ObjectStore for LocalFsObjectStore {
    fn put<'a>(
        &'a self,
        key: &'a str,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            self.ensure_parent(&path)?;
            tokio::fs::write(&path, data).await
        })
    }

    fn get<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            tokio::fs::read(&path).await
        })
    }

    fn exists<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<bool>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            Ok(path.exists())
        })
    }

    fn delete<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            match tokio::fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(()), // Already deleted
                Err(e) => Err(e),
            }
        })
    }

    fn list<'a>(
        &'a self,
        prefix: &'a str,
        _continuation_token: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = IoResult<ListResult>> + Send + 'a>> {
        Box::pin(async move {
            let base = self.base_path.clone();
            let prefix_path = if prefix.is_empty() {
                base.clone()
            } else {
                base.join(prefix)
            };

            // Get the directory to search
            let search_dir = if prefix_path.is_dir() {
                prefix_path.clone()
            } else {
                prefix_path.parent().unwrap_or(&base).to_path_buf()
            };

            if !search_dir.exists() {
                return Ok(ListResult::default());
            }

            let mut objects = Vec::new();
            let prefix_str = prefix.to_string();

            // Walk the directory
            fn walk_dir(
                dir: &PathBuf,
                base: &PathBuf,
                prefix: &str,
                objects: &mut Vec<ObjectMeta>,
            ) -> IoResult<()> {
                for entry in std::fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();

                    if path.is_dir() {
                        walk_dir(&path, base, prefix, objects)?;
                    } else if path.is_file() {
                        // TigerStyle: strip_prefix is safe - path is derived from walking base directory
                        let key = path
                            .strip_prefix(base)
                            .expect("path must be under base - we're walking base directory")
                            .to_string_lossy()
                            .to_string();

                        if key.starts_with(prefix) {
                            let metadata = std::fs::metadata(&path)?;
                            objects.push(ObjectMeta {
                                key,
                                size_bytes: metadata.len(),
                                created_at_ms: metadata
                                    .created()
                                    .ok()
                                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map(|d| d.as_millis() as u64)
                                    .unwrap_or(0),
                                etag: None,
                            });
                        }
                    }
                }
                Ok(())
            }

            walk_dir(&search_dir, &base, &prefix_str, &mut objects)?;
            objects.sort_by(|a, b| a.key.cmp(&b.key));

            Ok(ListResult {
                objects,
                continuation_token: None,
            })
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let from_path = self.full_path(from);
            let to_path = self.full_path(to);
            self.ensure_parent(&to_path)?;
            tokio::fs::rename(&from_path, &to_path).await
        })
    }

    fn head<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<ObjectMeta>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            let metadata = tokio::fs::metadata(&path).await?;
            Ok(ObjectMeta {
                key: key.to_string(),
                size_bytes: metadata.len(),
                created_at_ms: metadata
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                etag: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inmemory_put_get() {
        let store = InMemoryObjectStore::new();

        store.put("test/key1", b"hello world").await.unwrap();
        let data = store.get("test/key1").await.unwrap();

        assert_eq!(data, b"hello world");
    }

    #[tokio::test]
    async fn test_inmemory_exists() {
        let store = InMemoryObjectStore::new();

        assert!(!store.exists("test/key1").await.unwrap());
        store.put("test/key1", b"data").await.unwrap();
        assert!(store.exists("test/key1").await.unwrap());
    }

    #[tokio::test]
    async fn test_inmemory_delete() {
        let store = InMemoryObjectStore::new();

        store.put("test/key1", b"data").await.unwrap();
        assert!(store.exists("test/key1").await.unwrap());

        store.delete("test/key1").await.unwrap();
        assert!(!store.exists("test/key1").await.unwrap());
    }

    #[tokio::test]
    async fn test_inmemory_list() {
        let store = InMemoryObjectStore::new();

        store.put("segments/seg-001", b"data1").await.unwrap();
        store.put("segments/seg-002", b"data2").await.unwrap();
        store.put("checkpoints/chk-001", b"data3").await.unwrap();

        let result = store.list("segments/", None).await.unwrap();
        assert_eq!(result.objects.len(), 2);
        assert!(result
            .objects
            .iter()
            .all(|o| o.key.starts_with("segments/")));
    }

    #[tokio::test]
    async fn test_inmemory_rename() {
        let store = InMemoryObjectStore::new();

        store.put("old/key", b"data").await.unwrap();
        store.rename("old/key", "new/key").await.unwrap();

        assert!(!store.exists("old/key").await.unwrap());
        assert!(store.exists("new/key").await.unwrap());

        let data = store.get("new/key").await.unwrap();
        assert_eq!(data, b"data");
    }

    #[tokio::test]
    async fn test_inmemory_head() {
        let store = InMemoryObjectStore::new();

        let data = b"hello world";
        store.put("test/key", data).await.unwrap();

        let meta = store.head("test/key").await.unwrap();
        assert_eq!(meta.key, "test/key");
        assert_eq!(meta.size_bytes, data.len() as u64);
    }

    #[tokio::test]
    async fn test_localfs_put_get() {
        let store = LocalFsObjectStore::temp().unwrap();

        store.put("test/key1.txt", b"hello world").await.unwrap();
        let data = store.get("test/key1.txt").await.unwrap();

        assert_eq!(data, b"hello world");

        // Cleanup
        std::fs::remove_dir_all(store.base_path()).ok();
    }

    #[tokio::test]
    async fn test_localfs_list() {
        let store = LocalFsObjectStore::temp().unwrap();

        store.put("segments/seg-001.seg", b"data1").await.unwrap();
        store.put("segments/seg-002.seg", b"data2").await.unwrap();
        store
            .put("checkpoints/chk-001.chk", b"data3")
            .await
            .unwrap();

        let result = store.list("segments/", None).await.unwrap();
        assert_eq!(result.objects.len(), 2);

        // Cleanup
        std::fs::remove_dir_all(store.base_path()).ok();
    }

    #[tokio::test]
    async fn test_localfs_rename() {
        let store = LocalFsObjectStore::temp().unwrap();

        store.put("manifest.json.tmp", b"data").await.unwrap();
        store
            .rename("manifest.json.tmp", "manifest.json")
            .await
            .unwrap();

        assert!(!store.exists("manifest.json.tmp").await.unwrap());
        assert!(store.exists("manifest.json").await.unwrap());

        // Cleanup
        std::fs::remove_dir_all(store.base_path()).ok();
    }
}
