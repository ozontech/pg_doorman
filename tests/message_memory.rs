use std::io::Cursor;
use std::sync::atomic::Ordering;

use bytes::BytesMut;
use pg_doorman::errors::Error;
use pg_doorman::messages::{read_message, read_message_reuse, CURRENT_MEMORY};
use serial_test::serial;
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::time::{timeout, Duration};

fn message(len: i32) -> Vec<u8> {
    assert!(len >= 4);
    let mut message = vec![b'Q'];
    message.extend_from_slice(&len.to_be_bytes());
    message.resize(len as usize + 1, b'x');
    message
}

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
#[serial]
async fn cancelled_reads_release_memory() {
    let message = message(100);

    for mut buffer in [None, Some(BytesMut::new())] {
        for prefix_len in 0..message.len() {
            let (mut writer, mut reader) = tokio::io::duplex(message.len());
            writer.write_all(&message[..prefix_len]).await.unwrap();

            {
                // Pending must mean missing input, not an exhausted Tokio task budget.
                let mut pending = std::pin::pin!(tokio::task::unconstrained(read(
                    &mut reader,
                    buffer.as_mut(),
                    100
                )));
                assert!(futures::poll!(pending.as_mut()).is_pending());
                assert_eq!(
                    CURRENT_MEMORY.load(Ordering::Relaxed),
                    if prefix_len >= 5 { 100 } else { 0 },
                    "prefix length: {prefix_len}"
                );
            }

            assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);

            let result = read(&mut Cursor::new(&message), buffer.as_mut(), 100)
                .await
                .unwrap();
            assert_eq!(result.as_ref(), message.as_slice());
            assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);
        }
    }
}

#[tokio::test]
#[serial]
async fn completed_and_failed_reads_release_memory() {
    let message = message(100);

    for mut buffer in [None, Some(BytesMut::new())] {
        let result = read(&mut Cursor::new(&message), buffer.as_mut(), 100)
            .await
            .unwrap();
        assert_eq!(result.as_ref(), message.as_slice());
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);

        let result = read(&mut Cursor::new(&message), buffer.as_mut(), 99).await;
        assert!(matches!(result, Err(Error::CurrentMemoryUsage)));
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);

        let result = read(&mut Cursor::new(&message[..6]), buffer.as_mut(), 100).await;
        assert!(matches!(result, Err(Error::SocketError(_))));
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
#[serial]
async fn cancelling_one_read_preserves_other_reservations() {
    for reuse in [false, true] {
        let (mut first_writer, mut first_reader) = tokio::io::duplex(101);
        let (mut second_writer, mut second_reader) = tokio::io::duplex(101);
        first_writer.write_all(&message(60)[..5]).await.unwrap();
        second_writer.write_all(&message(100)[..5]).await.unwrap();
        let mut first_buffer = reuse.then(BytesMut::new);
        let mut second_buffer = reuse.then(BytesMut::new);
        let mut first = Box::pin(read(&mut first_reader, first_buffer.as_mut(), 160));
        let mut second = Box::pin(read(&mut second_reader, second_buffer.as_mut(), 160));

        assert!(futures::poll!(first.as_mut()).is_pending());
        assert!(futures::poll!(second.as_mut()).is_pending());
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 160);

        drop(first);
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 100);

        let mut next_buffer = reuse.then(BytesMut::new);
        let rejected = read(&mut Cursor::new(message(61)), next_buffer.as_mut(), 160).await;
        assert!(matches!(rejected, Err(Error::CurrentMemoryUsage)));
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 100);

        let next = message(60);
        assert_eq!(
            read(&mut Cursor::new(&next), next_buffer.as_mut(), 160)
                .await
                .unwrap()
                .as_ref(),
            next.as_slice()
        );
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 100);

        second_writer.write_all(&message(100)[5..]).await.unwrap();
        assert_eq!(second.await.unwrap().as_ref(), message(100));
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
#[serial]
async fn resumed_reads_keep_their_reservation() {
    let message = message(100);
    for mut buffer in [None, Some(BytesMut::new())] {
        let (mut writer, mut reader) = tokio::io::duplex(message.len());
        writer.write_all(&message[..6]).await.unwrap();
        let mut pending = std::pin::pin!(read(&mut reader, buffer.as_mut(), 100));

        tokio::select! {
            biased;
            result = &mut pending => panic!("incomplete read finished: {result:?}"),
            _ = std::future::ready(()) => {}
        }
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 100);

        writer.write_all(&message[6..]).await.unwrap();
        assert_eq!(pending.await.unwrap().as_ref(), message.as_slice());
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn aborted_tasks_release_memory() {
    for mut buffer in [None, Some(BytesMut::new())] {
        let (mut writer, mut reader) = tokio::io::duplex(101);
        writer.write_all(&message(100)[..6]).await.unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut pending = std::pin::pin!(read(&mut reader, buffer.as_mut(), 100));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            ready_tx.send(()).unwrap();
            pending.await
        });

        timeout(Duration::from_secs(5), ready_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 100);
        task.abort();
        let result = timeout(Duration::from_secs(5), task).await.unwrap();
        assert!(result.unwrap_err().is_cancelled());
        assert_eq!(CURRENT_MEMORY.load(Ordering::Relaxed), 0);
    }
}
