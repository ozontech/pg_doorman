//! Mock implementations for authentication testing.

use std::io::{Error as IoError, ErrorKind};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Mock implementation for AsyncReadExt
pub struct MockReader {
    data: Vec<Vec<u8>>,
    current_index: usize,
}

impl MockReader {
    pub fn new(data: Vec<Vec<u8>>) -> Self {
        Self {
            data,
            current_index: 0,
        }
    }
}

impl AsyncRead for MockReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), IoError>> {
        if self.current_index >= self.data.len() {
            return Poll::Ready(Err(IoError::new(ErrorKind::UnexpectedEof, "No more data")));
        }

        let data = &self.data[self.current_index];
        let to_copy = std::cmp::min(buf.remaining(), data.len());
        buf.put_slice(&data[..to_copy]);
        self.current_index += 1;

        Poll::Ready(Ok(()))
    }
}

/// Mock implementation for AsyncWriteExt
pub struct MockWriter {
    written: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl MockWriter {
    pub fn new() -> Self {
        Self {
            written: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[allow(dead_code)]
    pub fn get_written(&self) -> Vec<Vec<u8>> {
        self.written.lock().unwrap().clone()
    }
}

impl Default for MockWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncWrite for MockWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, IoError>> {
        self.written.lock().unwrap().push(buf.to_vec());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
        Poll::Ready(Ok(()))
    }
}

/// Helper struct for creating a Waker in tests
pub struct MockWaker;

impl Wake for MockWaker {
    fn wake(self: Arc<Self>) {}
}

pub fn get_waker() -> Waker {
    Arc::new(MockWaker).into()
}

pub async fn run_test<F, Fut, T>(f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let mut fut = Box::pin(f());
    let waker = get_waker();
    let mut cx = Context::from_waker(&waker);

    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(val) => val,
        Poll::Pending => panic!("Future is still pending"),
    }
}
