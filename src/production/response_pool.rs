//! Response Slot Pool - Reusable response mechanism for actor communication
//!
//! This replaces per-request oneshot::channel() allocations with a pool of
//! reusable response slots, reducing allocator pressure significantly.
//!
//! # Design
//!
//! Instead of creating a new oneshot channel pair for each request:
//! 1. Acquire a `ResponseSlot` from the pool
//! 2. Send the slot (wrapped in Arc) to the actor
//! 3. Actor writes response and calls `slot.send(value)`
//! 4. Caller awaits `slot.recv()` to get the response
//! 5. Return slot to pool for reuse
//!
//! # Performance
//!
//! - Eliminates channel allocation overhead (main bottleneck identified in profiling)
//! - Pool is lock-free using crossbeam::ArrayQueue
//! - Slots use parking_lot::Mutex for fast synchronization
//! - P6 optimization: Single lock for value+waker reduces contention

use crossbeam::queue::ArrayQueue;
use parking_lot::Mutex;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// P6: Combined state for single-lock optimization
struct SlotState<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

/// A reusable response slot that can hold a single value
///
/// Thread-safe and can be awaited from async code.
/// P6 optimization: Uses single lock for value+waker.
pub struct ResponseSlot<T> {
    /// Combined state under single lock (P6 optimization)
    state: Mutex<SlotState<T>>,
}

impl<T> std::fmt::Debug for ResponseSlot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseSlot")
            .field("has_value", &self.state.lock().value.is_some())
            .finish()
    }
}

impl<T> ResponseSlot<T> {
    /// Create a new empty response slot
    #[inline]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SlotState {
                value: None,
                waker: None,
            }),
        }
    }

    /// Send a response value to this slot
    ///
    /// This wakes any waiting receiver.
    /// P6 optimization: Single lock for value+waker.
    #[inline]
    pub fn send(&self, value: T) {
        let waker = {
            let mut state = self.state.lock();
            state.value = Some(value);
            state.waker.take()
        };
        // Wake outside the lock to avoid holding it during wake
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Reset the slot for reuse
    ///
    /// Must be called before returning to pool.
    /// P6 optimization: Single lock for value+waker.
    #[inline]
    pub fn reset(&self) {
        let mut state = self.state.lock();
        state.value = None;
        state.waker = None;
    }

    /// Take the value if ready (non-blocking)
    #[inline]
    #[allow(dead_code)]
    pub fn try_recv(&self) -> Option<T> {
        self.state.lock().value.take()
    }
}

impl<T> Default for ResponseSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Future that resolves when a response is available
pub struct ResponseFuture<T> {
    slot: Arc<ResponseSlot<T>>,
}

impl<T> Future for ResponseFuture<T> {
    type Output = T;

    /// P6 optimization: Single lock acquisition instead of 3
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.slot.state.lock();

        // Try to take the value
        if let Some(value) = state.value.take() {
            return Poll::Ready(value);
        }

        // Store waker for notification (value not ready yet)
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// A pool of reusable response slots
///
/// Lock-free acquisition and release using ArrayQueue.
pub struct ResponsePool<T> {
    pool: ArrayQueue<Arc<ResponseSlot<T>>>,
    #[allow(dead_code)]
    capacity: usize,
}

impl<T> ResponsePool<T> {
    /// Create a new pool with the given capacity
    ///
    /// Pre-allocates `prewarm` slots immediately.
    pub fn new(capacity: usize, prewarm: usize) -> Self {
        debug_assert!(prewarm <= capacity, "Prewarm count exceeds capacity");

        let pool = ArrayQueue::new(capacity);

        // Pre-warm the pool
        for _ in 0..prewarm {
            let slot = Arc::new(ResponseSlot::new());
            let _ = pool.push(slot);
        }

        Self { pool, capacity }
    }

    /// Acquire a response slot from the pool
    ///
    /// Returns a pooled slot if available, otherwise creates a new one.
    #[inline]
    pub fn acquire(&self) -> Arc<ResponseSlot<T>> {
        self.pool
            .pop()
            .unwrap_or_else(|| Arc::new(ResponseSlot::new()))
    }

    /// Release a slot back to the pool
    ///
    /// The slot is reset before being returned.
    /// If the pool is full, the slot is dropped.
    #[inline]
    pub fn release(&self, slot: Arc<ResponseSlot<T>>) {
        // Reset the slot for reuse
        slot.reset();

        // Try to return to pool (ignore if full)
        let _ = self.pool.push(slot);
    }

    /// Get the current number of pooled slots
    #[inline]
    #[allow(dead_code)]
    pub fn available(&self) -> usize {
        self.pool.len()
    }

    /// Get the pool capacity
    #[inline]
    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Helper to create a response future from a slot
#[inline]
pub fn response_future<T>(slot: Arc<ResponseSlot<T>>) -> ResponseFuture<T> {
    ResponseFuture { slot }
}

/// A handle that automatically returns the slot to the pool when dropped
pub struct PooledResponse<T> {
    slot: Option<Arc<ResponseSlot<T>>>,
    pool: Arc<ResponsePool<T>>,
}

impl<T> PooledResponse<T> {
    /// Create a new pooled response handle
    #[inline]
    pub fn new(slot: Arc<ResponseSlot<T>>, pool: Arc<ResponsePool<T>>) -> Self {
        Self {
            slot: Some(slot),
            pool,
        }
    }

    /// Get the underlying slot for sending to actor
    #[inline]
    pub fn slot(&self) -> Arc<ResponseSlot<T>> {
        self.slot.as_ref().expect("Slot already taken").clone()
    }

    /// Wait for the response
    #[inline]
    pub async fn recv(mut self) -> T {
        let slot = self.slot.take().expect("Slot already taken");
        let value = response_future(slot.clone()).await;
        self.pool.release(slot);
        value
    }
}

impl<T> Drop for PooledResponse<T> {
    fn drop(&mut self) {
        // Return slot to pool if not already taken
        if let Some(slot) = self.slot.take() {
            self.pool.release(slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_response_slot_send_recv() {
        let slot = Arc::new(ResponseSlot::new());

        // Send from another task
        let slot_clone = slot.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            slot_clone.send(42i32);
        });

        // Receive
        let value = response_future(slot).await;
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn test_response_slot_immediate() {
        let slot = Arc::new(ResponseSlot::new());

        // Send immediately
        slot.send(123i32);

        // Receive should return immediately
        let value = response_future(slot).await;
        assert_eq!(value, 123);
    }

    #[test]
    fn test_response_pool_acquire_release() {
        let pool: ResponsePool<i32> = ResponsePool::new(10, 5);

        // Should have 5 pre-warmed
        assert_eq!(pool.available(), 5);

        // Acquire all pre-warmed
        let slots: Vec<_> = (0..5).map(|_| pool.acquire()).collect();
        assert_eq!(pool.available(), 0);

        // Acquire more (creates new)
        let extra = pool.acquire();
        assert_eq!(pool.available(), 0);

        // Release all back
        for slot in slots {
            pool.release(slot);
        }
        pool.release(extra);

        // Should have 6 now (5 original + 1 extra)
        assert_eq!(pool.available(), 6);
    }

    #[tokio::test]
    async fn test_pooled_response() {
        let pool = Arc::new(ResponsePool::<i32>::new(10, 5));

        let slot = pool.acquire();
        let response = PooledResponse::new(slot.clone(), pool.clone());

        // Simulate actor sending response
        slot.send(999);

        // Receive and auto-return to pool
        let initial = pool.available();
        let value = response.recv().await;
        assert_eq!(value, 999);

        // Slot should be returned to pool
        assert_eq!(pool.available(), initial + 1);
    }
}
