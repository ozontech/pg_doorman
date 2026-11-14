//! Exercise the production roundtrip and backend decoder against a peer that
//! writes responses before consuming the remaining batch.

use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::time::timeout;

use crate::client::core::{
    BatchOperation, Client, ParseCompleteTarget, PreparedStatementState, SkippedParse,
};
use crate::messages::{ready_for_query, simple_query};
use crate::server::{Server, ServerParameters, StreamInner};
use crate::stats::ClientStats;

fn client<W>(write: W) -> Client<tokio::io::Empty, W> {
    Client {
        read: BufReader::new(tokio::io::empty()),
        write,
        buffer: Default::default(),
        addr: "127.0.0.1:12345".parse().unwrap(),
        addr_str: "127.0.0.1:12345".into(),
        read_buf: BytesMut::new(),
        connection_id: 1,
        cancel_mode: false,
        transaction_mode: false,
        secret_key: 0,
        client_server_map: Default::default(),
        stats: Arc::new(ClientStats::default()),
        admin: false,
        last_server_stats: None,
        connected_to_server: true,
        session_xact_start: None,
        pool_name: "roundtrip_test".into(),
        username: "roundtrip_test".into(),
        server_parameters: ServerParameters::new(),
        prepared: PreparedStatementState::default(),
        max_memory_usage: u64::MAX,
        client_last_messages_in_tx: Default::default(),
        client_pending_begin: None,
        #[cfg(unix)]
        raw_fd: None,
        #[cfg(all(unix, feature = "tls-migration"))]
        ssl_ptr: None,
    }
}

fn frame(code: u8, body: &[u8]) -> BytesMut {
    let mut bytes = BytesMut::new();
    bytes.put_u8(code);
    bytes.put_i32((body.len() + 4) as i32);
    bytes.extend_from_slice(body);
    bytes
}

fn parse(query: &str) -> BytesMut {
    let mut body = BytesMut::new();
    body.put_u8(0); // unnamed statement
    body.extend_from_slice(query.as_bytes());
    body.put_u8(0);
    body.put_i16(0);
    frame(b'P', &body)
}

fn batch() -> BytesMut {
    let mut batch = parse("SELECT repeat('x', 524288)");
    batch.extend_from_slice(&frame(b'B', &[0; 8]));
    batch.extend_from_slice(&frame(b'E', &[0; 5]));
    batch.extend_from_slice(&parse(&format!(
        "SELECT 1 /* {} */",
        "x".repeat(1024 * 1024)
    )));
    batch.extend_from_slice(&frame(b'B', &[0; 8]));
    batch.extend_from_slice(&frame(b'E', &[0; 5]));
    batch.extend_from_slice(&frame(b'S', &[]));
    batch
}

fn large_row() -> BytesMut {
    let mut body = BytesMut::new();
    body.put_i16(1);
    body.put_i32(512 * 1024);
    body.resize(body.len() + 512 * 1024, b'x');
    frame(b'D', &body)
}

fn sql_error() -> BytesMut {
    frame(b'E', b"SERROR\0VERROR\0C22000\0Mtest SQL error\0\0")
}

async fn read_frame<S: AsyncRead + Unpin>(peer: &mut S) -> BytesMut {
    let code = peer.read_u8().await.unwrap();
    let len = peer.read_u32().await.unwrap() as usize;
    assert!(len >= 4);
    let mut body = vec![0; len - 4];
    peer.read_exact(&mut body).await.unwrap();
    frame(code, &body)
}

async fn large_exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut server: Server,
    mut peer: S,
    copy_out: bool,
    cached_parse: bool,
) {
    let mut request = batch();
    if cached_parse {
        let len = u32::from_be_bytes(request[1..5].try_into().unwrap()) as usize;
        let _ = request.split_to(len + 1);
    }
    let mut first = if cached_parse {
        BytesMut::new()
    } else {
        frame(b'1', &[])
    };
    first.extend_from_slice(&frame(b'2', &[]));
    if copy_out {
        first.extend_from_slice(&frame(b'H', &[0, 0, 0]));
        first.extend_from_slice(&frame(b'd', &vec![b'x'; 512 * 1024]));
        first.extend_from_slice(&frame(b'c', &[]));
        first.extend_from_slice(&frame(b'C', b"COPY 1\0"));
    } else {
        first.extend_from_slice(&large_row());
        first.extend_from_slice(&frame(b'C', b"SELECT 1\0"));
    }
    let mut tail = frame(b'1', &[]);
    tail.extend_from_slice(&frame(b'2', &[]));
    tail.extend_from_slice(&frame(b'C', b"SELECT 1\0"));
    tail.extend_from_slice(&ready_for_query(false));
    let mut expected = BytesMut::new();
    if cached_parse {
        expected.extend_from_slice(&frame(b'1', &[]));
    }
    expected.extend_from_slice(&first);
    expected.extend_from_slice(&tail);
    expected.extend_from_slice(&ready_for_query(false)); // second roundtrip

    let (output, mut application) = tokio::io::duplex(64);
    let mut client = client(output);
    client.buffer.extend_from_slice(&request);
    if cached_parse {
        client.prepared.skipped_parses.push(SkippedParse {
            statement_name: "cached".into(),
            target: ParseCompleteTarget::BindComplete,
            insert_at_beginning: false,
            has_bind: true,
        });
        client.prepared.batch_operations = vec![
            BatchOperation::ParseSkipped {
                statement_name: "cached".into(),
            },
            BatchOperation::Bind {
                statement_name: "cached".into(),
            },
            BatchOperation::Execute,
            BatchOperation::ParseSent {
                statement_name: "new".into(),
            },
            BatchOperation::Bind {
                statement_name: "new".into(),
            },
            BatchOperation::Execute,
        ];
    }
    let backend = async {
        let first_codes: &[u8] = if cached_parse { b"BE" } else { b"PBE" };
        for &code in first_codes {
            assert_eq!(read_frame(&mut peer).await[0], code);
        }
        peer.write_all(&first).await.unwrap();
        // This cannot be reached with the original send-then-receive loop:
        // both the 1 MiB request and the 512 KiB response exceed the buffers.
        for code in [b'P', b'B', b'E', b'S'] {
            assert_eq!(read_frame(&mut peer).await[0], code);
        }
        peer.write_all(&tail).await.unwrap();
        assert_eq!(read_frame(&mut peer).await, simple_query(";"));
        peer.write_all(&ready_for_query(false)).await.unwrap();
    };
    let exchange = async {
        client
            .execute_server_roundtrip(None, &mut server)
            .await
            .unwrap();
        assert!(!server.is_bad());
        assert!(!server.roundtrip_in_progress);
        assert!(!server.in_copy_mode());
        assert!(!server.is_data_available());
        assert_eq!(&client.buffer[..], &request[..]);
        client.buffer.clear();
        client.prepared.reset_batch();
        server.checkin_cleanup().await.unwrap();
        // The same backend must decode the next response without leftovers.
        client
            .execute_server_roundtrip(Some(&simple_query(";")), &mut server)
            .await
            .unwrap();
        assert!(!server.is_bad());
    };
    let receive = async {
        let mut received = vec![0; expected.len()];
        application.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected);
    };
    timeout(Duration::from_secs(3), async {
        tokio::join!(backend, exchange, receive);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unix_batch_streams_large_rows_and_reuses_backend() {
    let (transport, peer) = UnixStream::pair().unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    socket2::SockRef::from(&peer)
        .set_send_buffer_size(1024)
        .unwrap();
    large_exchange(
        Server::for_test(StreamInner::UnixSocket { stream: transport }),
        peer,
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn tcp_batch_streams_large_rows_and_reuses_backend() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let connect = TcpStream::connect(listener.local_addr().unwrap());
    let (transport, peer) = tokio::join!(connect, listener.accept());
    let transport = transport.unwrap();
    let (peer, _) = peer.unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    socket2::SockRef::from(&peer)
        .set_send_buffer_size(1024)
        .unwrap();
    large_exchange(
        Server::for_test(StreamInner::TCPPlain { stream: transport }),
        peer,
        false,
        false,
    )
    .await;
}

#[tokio::test]
async fn copy_out_keeps_reading_while_the_batch_is_being_written() {
    let (transport, peer) = UnixStream::pair().unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    socket2::SockRef::from(&peer)
        .set_send_buffer_size(1024)
        .unwrap();
    large_exchange(
        Server::for_test(StreamInner::UnixSocket { stream: transport }),
        peer,
        true,
        false,
    )
    .await;
}

#[tokio::test]
async fn synthetic_parse_complete_precedes_a_streamed_data_row() {
    let (transport, peer) = UnixStream::pair().unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    socket2::SockRef::from(&peer)
        .set_send_buffer_size(1024)
        .unwrap();
    large_exchange(
        Server::for_test(StreamInner::UnixSocket { stream: transport }),
        peer,
        false,
        true,
    )
    .await;
}

#[tokio::test]
async fn flush_error_does_not_cancel_the_remaining_batch() {
    let (transport, mut peer) = UnixStream::pair().unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    let mut server = Server::for_test(StreamInner::UnixSocket { stream: transport });
    server.session_mode = true;
    server.set_async_mode(true);
    server.set_expected_responses(3);
    let (output, mut application) = tokio::io::duplex(64);
    let mut client = client(output);
    let mut request = batch();
    let last = request.len() - 5;
    request[last] = b'H';
    client.buffer.extend_from_slice(&request);
    let expected = sql_error();
    let backend = async {
        for _ in 0..3 {
            read_frame(&mut peer).await;
        }
        peer.write_all(&expected).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        for code in [b'P', b'B', b'E', b'H'] {
            assert_eq!(read_frame(&mut peer).await[0], code);
        }
    };
    let exchange = async {
        client
            .execute_server_roundtrip(None, &mut server)
            .await
            .unwrap();
        assert!(!server.roundtrip_in_progress);
        assert!(!server.is_bad());
    };
    let receive = async {
        let mut received = vec![0; expected.len()];
        application.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected);
    };
    timeout(Duration::from_secs(2), async {
        tokio::join!(backend, exchange, receive);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cancellation_after_flush_error_never_returns_backend_to_pool() {
    let (transport, mut peer) = UnixStream::pair().unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    let mut server = Server::for_test(StreamInner::UnixSocket { stream: transport });
    server.session_mode = true;
    server.set_async_mode(true);
    server.set_expected_responses(3);
    let (output, mut application) = tokio::io::duplex(64);
    let mut client = client(output);
    client.buffer.extend_from_slice(&batch());
    let expected = sql_error();
    let backend = async {
        for _ in 0..3 {
            read_frame(&mut peer).await;
        }
        peer.write_all(&expected).await.unwrap();
    };
    // The small error fits in the backend socket, but the remaining request
    // cannot fit. Wait until the reader forwards the error, then cancel.
    timeout(Duration::from_secs(2), async {
        let cancel = async {
            tokio::select! {
                result = client.execute_server_roundtrip(None, &mut server) => panic!("roundtrip unexpectedly completed: {result:?}"),
                _ = async {
                    let mut received = vec![0; expected.len()];
                    application.read_exact(&mut received).await.unwrap();
                    assert_eq!(received, expected);
                } => {}
            }
        };
        tokio::join!(backend, cancel);
    }).await.unwrap();
    assert!(server.roundtrip_in_progress);
    assert!(server.is_bad());
    assert!(server.checkin_cleanup().await.is_err());
    assert!(client
        .execute_server_roundtrip(Some(&frame(b'S', &[])), &mut server)
        .await
        .is_err());
}

#[tokio::test]
async fn client_disconnect_after_a_complete_send_drains_and_reuses_backend() {
    let (transport, mut peer) = UnixStream::pair().unwrap();
    let mut server = Server::for_test(StreamInner::UnixSocket { stream: transport });
    let (output, application) = tokio::io::duplex(64);
    drop(application);
    let mut client = client(output);
    client.transaction_mode = true;

    let mut row = BytesMut::new();
    row.put_i16(1);
    row.put_i32(512);
    row.resize(row.len() + 512, b'x');
    let row = frame(b'D', &row);
    let backend = async {
        assert_eq!(read_frame(&mut peer).await, simple_query("SELECT 1"));
        // Force a buffered response flush before ReadyForQuery. Every frame
        // stays below the large-message streaming threshold.
        for _ in 0..20 {
            peer.write_all(&row).await.unwrap();
        }
        peer.write_all(&frame(b'C', b"SELECT 20\0")).await.unwrap();
        peer.write_all(&ready_for_query(false)).await.unwrap();
        assert_eq!(read_frame(&mut peer).await, simple_query(";"));
        peer.write_all(&ready_for_query(false)).await.unwrap();
    };
    let exchange = async {
        client
            .execute_server_roundtrip(Some(&simple_query("SELECT 1")), &mut server)
            .await
            .unwrap();
        assert!(!server.is_bad());
        assert!(!server.is_data_available());
        server.checkin_cleanup().await.unwrap();
        client
            .execute_server_roundtrip(Some(&simple_query(";")), &mut server)
            .await
            .unwrap();
        assert!(!server.is_bad());
    };
    timeout(Duration::from_secs(1), async {
        tokio::join!(backend, exchange);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn client_disconnect_does_not_reuse_backend_when_draining_fails() {
    let (transport, mut peer) = UnixStream::pair().unwrap();
    let mut server = Server::for_test(StreamInner::UnixSocket { stream: transport });
    server.max_message_size = 0;
    let (output, application) = tokio::io::duplex(64);
    drop(application);
    let mut client = client(output);
    client.transaction_mode = true;

    let backend = async move {
        read_frame(&mut peer).await;
        // The first response flush hits the disconnected client. Then the
        // backend closes without ReadyForQuery, so draining must fail.
        peer.write_all(&large_row()).await.unwrap();
    };
    let exchange = async {
        assert!(client
            .execute_server_roundtrip(Some(&simple_query("SELECT 1")), &mut server)
            .await
            .is_err());
        assert!(server.is_bad());
        assert!(server.checkin_cleanup().await.is_err());
    };
    timeout(Duration::from_secs(1), async {
        tokio::join!(backend, exchange);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn client_disconnect_closes_backend_instead_of_waiting_to_drain() {
    let (transport, mut peer) = UnixStream::pair().unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    socket2::SockRef::from(&peer)
        .set_send_buffer_size(1024)
        .unwrap();
    let mut server = Server::for_test(StreamInner::UnixSocket { stream: transport });
    let (output, application) = tokio::io::duplex(64);
    drop(application);
    let mut client = client(output);
    client.buffer.extend_from_slice(&batch());
    let backend = async {
        for _ in 0..3 {
            read_frame(&mut peer).await;
        }
        peer.write_all(&large_row()).await.unwrap();
        std::future::pending::<()>().await;
    };
    timeout(Duration::from_secs(1), async {
        tokio::select! {
            result = client.execute_server_roundtrip(None, &mut server) => assert!(result.is_err()),
            _ = backend => unreachable!(),
        }
    })
    .await
    .unwrap();
    assert!(server.is_bad());
}

#[tokio::test]
async fn pending_close_complete_is_inserted_before_ready_for_query() {
    let (transport, mut peer) = UnixStream::pair().unwrap();
    let mut server = Server::for_test(StreamInner::UnixSocket { stream: transport });
    let (output, mut application) = tokio::io::duplex(64);
    let mut client = client(output);
    client.prepared.pending_close_complete = 1;
    let mut response = frame(b'3', &[]);
    response.extend_from_slice(&ready_for_query(false));
    let mut expected = frame(b'3', &[]);
    expected.extend_from_slice(&response);
    let backend = async {
        read_frame(&mut peer).await;
        peer.write_all(&response).await.unwrap();
    };
    let exchange = async {
        client
            .execute_server_roundtrip(Some(&frame(b'S', &[])), &mut server)
            .await
            .unwrap();
        assert_eq!(client.prepared.pending_close_complete, 0);
    };
    let receive = async {
        let mut received = vec![0; expected.len()];
        application.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected);
    };
    timeout(Duration::from_secs(1), async {
        tokio::join!(backend, exchange, receive);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn fast_release_waits_for_the_pending_writer() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let (transport, mut peer) = UnixStream::pair().unwrap();
    socket2::SockRef::from(&transport)
        .set_send_buffer_size(1024)
        .unwrap();
    let mut server = Server::for_test(StreamInner::UnixSocket { stream: transport });
    let (output, _application) = tokio::io::duplex(64);
    let mut client = client(output);
    client.transaction_mode = true;
    client.buffer.extend_from_slice(&batch());
    let accepting_tail = AtomicBool::new(false);
    let backend = async {
        for _ in 0..3 {
            read_frame(&mut peer).await;
        }
        peer.write_all(&ready_for_query(false)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        accepting_tail.store(true, Ordering::Relaxed);
        for _ in 0..4 {
            read_frame(&mut peer).await;
        }
    };
    let exchange = async {
        client
            .execute_server_roundtrip(None, &mut server)
            .await
            .unwrap();
        assert!(accepting_tail.load(Ordering::Relaxed));
        assert!(!server.roundtrip_in_progress);
        assert_eq!(
            &client.client_last_messages_in_tx[..],
            &ready_for_query(false)[..]
        );
    };
    timeout(Duration::from_secs(1), async {
        tokio::join!(backend, exchange);
    })
    .await
    .unwrap();
}
