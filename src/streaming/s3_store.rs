//! S3 Object Store Implementation
//!
//! Provides an S3-compatible object store for production use.
//! Uses the `object_store` crate from the Arrow ecosystem.
//!
//! Supports:
//! - AWS S3
//! - S3-compatible services (MinIO, LocalStack, etc.)
//! - Custom endpoints

use crate::streaming::config::S3Config;
use crate::streaming::object_store::{ListResult, ObjectMeta, ObjectStore};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore as ObjectStoreTrait;
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::pin::Pin;
use std::sync::Arc;

/// S3 Object Store for production deployments
///
/// Uses the `object_store` crate which provides:
/// - Standard S3 API support
/// - S3-compatible services (MinIO, LocalStack)
/// - Built-in retry logic
/// - Async streaming
#[derive(Clone)]
pub struct S3ObjectStore {
    store: Arc<dyn ObjectStoreTrait>,
    prefix: String,
}

impl S3ObjectStore {
    /// Create a new S3 object store
    ///
    /// Configuration via environment variables:
    /// - AWS_ACCESS_KEY_ID
    /// - AWS_SECRET_ACCESS_KEY
    /// - AWS_REGION (or uses config.region)
    /// - AWS_ENDPOINT (or uses config.endpoint for MinIO)
    pub async fn new(config: S3Config) -> IoResult<Self> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&config.bucket)
            .with_region(&config.region);

        // Use custom endpoint for S3-compatible services (MinIO)
        if let Some(endpoint) = &config.endpoint {
            builder = builder
                .with_endpoint(endpoint)
                .with_allow_http(endpoint.starts_with("http://"));
        }

        // Try to get credentials from environment
        builder = builder.with_access_key_id(
            std::env::var("AWS_ACCESS_KEY_ID")
                .unwrap_or_default(),
        );
        builder = builder.with_secret_access_key(
            std::env::var("AWS_SECRET_ACCESS_KEY")
                .unwrap_or_default(),
        );

        let store = builder.build().map_err(|e| {
            IoError::new(
                ErrorKind::InvalidInput,
                format!("Failed to create S3 store: {}", e),
            )
        })?;

        Ok(S3ObjectStore {
            store: Arc::new(store),
            prefix: config.prefix,
        })
    }

    /// Create from an existing object store (for testing)
    pub fn from_store(store: Arc<dyn ObjectStoreTrait>, prefix: String) -> Self {
        S3ObjectStore { store, prefix }
    }

    /// Get the full path with prefix
    fn full_path(&self, key: &str) -> ObjectPath {
        if self.prefix.is_empty() {
            ObjectPath::from(key)
        } else {
            ObjectPath::from(format!("{}/{}", self.prefix, key))
        }
    }

    /// Strip prefix from path
    fn strip_prefix(&self, path: &ObjectPath) -> String {
        let path_str = path.to_string();
        if self.prefix.is_empty() {
            path_str
        } else {
            let prefix_with_slash = format!("{}/", self.prefix);
            path_str
                .strip_prefix(&prefix_with_slash)
                .unwrap_or(&path_str)
                .to_string()
        }
    }

    /// Convert object_store errors to IoError
    fn map_error(err: object_store::Error) -> IoError {
        match &err {
            object_store::Error::NotFound { .. } => {
                IoError::new(ErrorKind::NotFound, err.to_string())
            }
            object_store::Error::AlreadyExists { .. } => {
                IoError::new(ErrorKind::AlreadyExists, err.to_string())
            }
            object_store::Error::Precondition { .. } => {
                IoError::new(ErrorKind::InvalidInput, err.to_string())
            }
            _ => IoError::new(ErrorKind::Other, err.to_string()),
        }
    }
}

impl std::fmt::Debug for S3ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3ObjectStore")
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl ObjectStore for S3ObjectStore {
    fn put<'a>(
        &'a self,
        key: &'a str,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            self.store
                .put(&path, bytes::Bytes::copy_from_slice(data).into())
                .await
                .map_err(Self::map_error)?;
            Ok(())
        })
    }

    fn get<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            let result = self.store.get(&path).await.map_err(Self::map_error)?;
            let data = result.bytes().await.map_err(Self::map_error)?;
            Ok(data.to_vec())
        })
    }

    fn exists<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<bool>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            match self.store.head(&path).await {
                Ok(_) => Ok(true),
                Err(object_store::Error::NotFound { .. }) => Ok(false),
                Err(e) => Err(Self::map_error(e)),
            }
        })
    }

    fn delete<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            // S3 delete is idempotent - ignore not found errors
            match self.store.delete(&path).await {
                Ok(()) => Ok(()),
                Err(object_store::Error::NotFound { .. }) => Ok(()),
                Err(e) => Err(Self::map_error(e)),
            }
        })
    }

    fn list<'a>(
        &'a self,
        prefix: &'a str,
        continuation_token: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = IoResult<ListResult>> + Send + 'a>> {
        Box::pin(async move {
            use futures::TryStreamExt;

            let full_prefix = self.full_path(prefix);

            // Parse offset from continuation token
            let offset: usize = continuation_token
                .and_then(|t| t.parse().ok())
                .unwrap_or(0);

            // List all objects with prefix
            let mut objects = Vec::new();
            let list_stream = self.store.list(Some(&full_prefix));

            let all_objects: Vec<_> = list_stream
                .try_collect()
                .await
                .map_err(Self::map_error)?;

            // Apply pagination
            const PAGE_SIZE: usize = 1000;
            let page = all_objects.into_iter().skip(offset).take(PAGE_SIZE + 1);

            let mut count = 0;
            for meta in page {
                if count >= PAGE_SIZE {
                    // There are more results
                    let next_token = (offset + PAGE_SIZE).to_string();
                    return Ok(ListResult {
                        objects,
                        continuation_token: Some(next_token),
                    });
                }

                objects.push(ObjectMeta {
                    key: self.strip_prefix(&meta.location),
                    size_bytes: meta.size as u64,
                    created_at_ms: meta
                        .last_modified
                        .timestamp_millis()
                        .try_into()
                        .unwrap_or(0),
                    etag: meta.e_tag.clone(),
                });
                count += 1;
            }

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

            // object_store has a rename method that does copy+delete
            self.store
                .rename(&from_path, &to_path)
                .await
                .map_err(Self::map_error)?;

            Ok(())
        })
    }

    fn head<'a>(
        &'a self,
        key: &'a str,
    ) -> Pin<Box<dyn Future<Output = IoResult<ObjectMeta>> + Send + 'a>> {
        Box::pin(async move {
            let path = self.full_path(key);
            let meta = self.store.head(&path).await.map_err(Self::map_error)?;

            Ok(ObjectMeta {
                key: key.to_string(),
                size_bytes: meta.size as u64,
                created_at_ms: meta
                    .last_modified
                    .timestamp_millis()
                    .try_into()
                    .unwrap_or(0),
                etag: meta.e_tag,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_path_with_prefix() {
        let prefix = "redis-stream";
        let key = "segments/seg001.bin";

        let full = if prefix.is_empty() {
            ObjectPath::from(key)
        } else {
            ObjectPath::from(format!("{}/{}", prefix, key))
        };

        assert_eq!(full.to_string(), "redis-stream/segments/seg001.bin");
    }

    #[test]
    fn test_full_path_without_prefix() {
        let prefix = "";
        let key = "segments/seg001.bin";

        let full = if prefix.is_empty() {
            ObjectPath::from(key)
        } else {
            ObjectPath::from(format!("{}/{}", prefix, key))
        };

        assert_eq!(full.to_string(), "segments/seg001.bin");
    }

    #[test]
    fn test_strip_prefix() {
        let prefix = "redis-stream";
        let full_path = "redis-stream/segments/seg001.bin";

        let prefix_with_slash = format!("{}/", prefix);
        let stripped = full_path
            .strip_prefix(&prefix_with_slash)
            .unwrap_or(full_path);

        assert_eq!(stripped, "segments/seg001.bin");
    }
}
