use bytes::BytesMut;
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};

const DEFAULT_BUFFER_CAPACITY: usize = 8192;
const BUFFER_SHRINK_THRESHOLD: usize = 4 * DEFAULT_BUFFER_CAPACITY; // 32KB
const MAX_POOL_SIZE: usize = 512; // ~5MB per thread

thread_local! {
    static LOCAL_POOL: RefCell<Vec<BytesMut>> = RefCell::new(Vec::with_capacity(MAX_POOL_SIZE));
}

/// Acquire a buffer from the thread-local pool or create a new one.
#[inline]
fn acquire_buffer() -> BytesMut {
    LOCAL_POOL.with(|pool| {
        pool.borrow_mut()
            .pop()
            .unwrap_or_else(|| BytesMut::with_capacity(DEFAULT_BUFFER_CAPACITY))
    })
}

/// Return a buffer to the thread-local pool.
/// If the buffer is too large, it is dropped instead to reclaim memory.
#[inline]
fn release_buffer(mut buffer: BytesMut) {
    if buffer.capacity() > BUFFER_SHRINK_THRESHOLD {
        // Drop it, don't pollute the pool with huge buffers
        return;
    }

    // Clear content but keep capacity
    buffer.clear();

    LOCAL_POOL.with(|pool| {
        if let Ok(mut pool) = pool.try_borrow_mut() {
            if pool.len() < MAX_POOL_SIZE {
                pool.push(buffer);
            }
        }
        // If borrow fails or pool is full, just drop the buffer
    })
}

/// RAII wrapper for BytesMut that returns it to the pool on Drop.
#[derive(Debug)]
pub struct PooledBuffer(Option<BytesMut>);

impl PooledBuffer {
    #[inline]
    pub fn new() -> Self {
        Self(Some(acquire_buffer()))
    }

    /// Checks if the buffer capacity exceeds the threshold.
    /// If so, replaces the underlying buffer with a new standard-sized one from the pool.
    /// The old large buffer is dropped (releasing memory).
    #[inline]
    pub fn shrink_if_needed(&mut self) {
        // self.0 is always Some during normal usage
        if self.capacity() > BUFFER_SHRINK_THRESHOLD {
            self.0 = Some(acquire_buffer());
        }
    }
}

impl Default for PooledBuffer {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for PooledBuffer {
    type Target = BytesMut;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl DerefMut for PooledBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
    }
}

impl Drop for PooledBuffer {
    #[inline]
    fn drop(&mut self) {
        if let Some(buffer) = self.0.take() {
            release_buffer(buffer);
        }
    }
}
