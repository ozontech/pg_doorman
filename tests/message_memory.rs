use std::io::Cursor;
use std::sync::atomic::Ordering;

use bytes::BytesMut;
use pg_doorman::errors::Error;
use pg_doorman::messages::{read_message, read_message_reuse, CURRENT_MEMORY};
use tokio::io::{AsyncRead, AsyncWriteExt};

async fn read<S: AsyncRead + Unpin>(
    stream: &mut S,
    buffer: Option<&mut BytesMut>,
    limit: u64,
) -> Result<BytesMut, Error> {
    match buffer {
        Some(buffer) => read_message_reuse(stream, buffer, limit).await,
        None => read_message(stream, limit).await,
    }
}

// A separate test binary keeps CURRENT_MEMORY isolated from other socket tests.
#[tokio::test]
async fn cancelled_reads_release_memory() {
    let mut message = vec![b'Q'];
    message.extend_from_slice(&100_i32.to_be_bytes());
    message.extend_from_slice(&[b'x'; 96]);

    for mut buffer in [None, Some(BytesMut::new())] {
        for prefix_len in [0, 1, 3, 5, 6, message.len() - 1] {
            let (mut writer, mut reader) = tokio::io::duplex(message.len());
            writer.write_all(&message[..prefix_len]).await.unwrap();

            {
                let mut pending = std::pin::pin!(read(&mut reader, buffer.as_mut(), 100));
                assert!(futures::poll!(pending.as_mut()).is_pending());
                assert_eq!(
                    CURRENT_MEMORY.load(Ordering::Relaxed),
                    if prefix_len >= 5 { 100 } else { 0 }
                );
            }

            assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);

            let result = read(&mut Cursor::new(&message), buffer.as_mut(), 100)
                .await
                .unwrap();
            assert_eq!(result.as_ref(), message.as_slice());
            assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);
        }

        let result = read(&mut Cursor::new(&message), buffer.as_mut(), 99).await;
        assert!(matches!(result, Err(Error::CurrentMemoryUsage)));
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);

        let result = read(&mut Cursor::new(&message[..6]), buffer.as_mut(), 100).await;
        assert!(matches!(result, Err(Error::SocketError(_))));
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);
    }
}
