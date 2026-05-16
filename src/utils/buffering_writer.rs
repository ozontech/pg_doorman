//! `AsyncWrite` over `&mut BytesMut`. Used to capture bytes that would otherwise
//! be proxied to a client socket — feeds `Server::recv` and lets the caller
//! inspect or cache the full response without touching the client connection.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::BytesMut;
use tokio::io::AsyncWrite;

pub struct BufferingWriter<'a> {
    buf: &'a mut BytesMut,
}

impl<'a> BufferingWriter<'a> {
    pub fn new(buf: &'a mut BytesMut) -> Self {
        Self { buf }
    }
}

impl<'a> AsyncWrite for BufferingWriter<'a> {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        self.get_mut().buf.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn write_all_then_flush_appends_to_buffer() {
        let mut buf = BytesMut::new();
        {
            let mut writer = BufferingWriter::new(&mut buf);
            writer.write_all(b"hello").await.unwrap();
            writer.flush().await.unwrap();
        }
        assert_eq!(&buf[..], b"hello");
    }

    #[tokio::test]
    async fn multi_chunk_writes_concatenate_in_order() {
        let mut buf = BytesMut::new();
        {
            let mut writer = BufferingWriter::new(&mut buf);
            writer.write_all(b"foo").await.unwrap();
            writer.write_all(b"bar").await.unwrap();
            writer.write_all(b"baz").await.unwrap();
        }
        assert_eq!(&buf[..], b"foobarbaz");
    }

    #[tokio::test]
    async fn shutdown_is_idempotent_and_preserves_buffer() {
        let mut buf = BytesMut::new();
        {
            let mut writer = BufferingWriter::new(&mut buf);
            writer.write_all(b"payload").await.unwrap();
            writer.shutdown().await.unwrap();
            writer.shutdown().await.unwrap();
        }
        assert_eq!(&buf[..], b"payload");
    }

    #[tokio::test]
    async fn empty_write_keeps_buffer_empty() {
        let mut buf = BytesMut::new();
        {
            let mut writer = BufferingWriter::new(&mut buf);
            writer.write_all(b"").await.unwrap();
        }
        assert!(buf.is_empty());
    }
}
