use bytes::BytesMut;
use crossbeam::queue::ArrayQueue;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Error returned when connection pool operations fail
#[derive(Debug)]
pub enum ConnectionPoolError {
    /// Semaphore was closed (typically during shutdown)
    SemaphoreClosed,
}

impl std::fmt::Display for ConnectionPoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionPoolError::SemaphoreClosed => {
                write!(f, "Connection pool semaphore closed")
            }
        }
    }
}

impl std::error::Error for ConnectionPoolError {}

pub struct ConnectionPool {
    buffer_pool: Arc<BufferPoolAsync>,
    max_connections: Arc<Semaphore>,
}

impl ConnectionPool {
    pub fn new(max_connections: usize, buffer_pool_size: usize) -> Self {
        ConnectionPool {
            buffer_pool: Arc::new(BufferPoolAsync::new(buffer_pool_size, 8192)),
            max_connections: Arc::new(Semaphore::new(max_connections)),
        }
    }

    /// TigerStyle: Return Result instead of panicking on semaphore close
    pub async fn acquire_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, ConnectionPoolError> {
        self.max_connections
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ConnectionPoolError::SemaphoreClosed)
    }

    pub fn acquire_buffer(&self) -> BytesMut {
        self.buffer_pool.acquire()
    }

    pub fn release_buffer(&self, buf: BytesMut) {
        self.buffer_pool.release(buf);
    }

    pub fn buffer_pool(&self) -> Arc<BufferPoolAsync> {
        self.buffer_pool.clone()
    }
}

pub struct BufferPoolAsync {
    pool: ArrayQueue<BytesMut>,
    capacity: usize,
}

impl BufferPoolAsync {
    pub fn new(size: usize, buffer_capacity: usize) -> Self {
        let pool = ArrayQueue::new(size);
        for _ in 0..size {
            let _ = pool.push(BytesMut::with_capacity(buffer_capacity));
        }
        BufferPoolAsync {
            pool,
            capacity: buffer_capacity,
        }
    }

    pub fn acquire(&self) -> BytesMut {
        self.pool
            .pop()
            .unwrap_or_else(|| BytesMut::with_capacity(self.capacity))
    }

    pub fn release(&self, mut buf: BytesMut) {
        buf.clear();
        if buf.capacity() <= self.capacity * 2 {
            let _ = self.pool.push(buf);
        }
    }
}

impl Default for ConnectionPool {
    fn default() -> Self {
        Self::new(10000, 64)
    }
}
