use std::sync::Arc;
use crossbeam::queue::ArrayQueue;
use bytes::BytesMut;
use tokio::sync::Semaphore;

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

    pub async fn acquire_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.max_connections.clone().acquire_owned().await.unwrap()
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
        BufferPoolAsync { pool, capacity: buffer_capacity }
    }

    pub fn acquire(&self) -> BytesMut {
        self.pool.pop().unwrap_or_else(|| BytesMut::with_capacity(self.capacity))
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
        Self::new(10000, 512)
    }
}
