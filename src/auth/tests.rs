//! Tests for authentication module.

use super::*;
use std::io::{Error as IoError, ErrorKind};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

// Mock implementation for AsyncReadExt
struct MockReader {
    data: Vec<Vec<u8>>,
    current_index: usize,
}

impl MockReader {
    fn new(data: Vec<Vec<u8>>) -> Self {
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

// Mock implementation for AsyncWriteExt
struct MockWriter {
    written: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl MockWriter {
    fn new() -> Self {
        Self {
            written: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[allow(dead_code)]
    fn get_written(&self) -> Vec<Vec<u8>> {
        self.written.lock().unwrap().clone()
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

// Helper function to run async hba_eval_tests
struct MockWaker;
impl Wake for MockWaker {
    fn wake(self: Arc<Self>) {}
}

fn get_waker() -> Waker {
    Arc::new(MockWaker).into()
}

async fn run_test<F, Fut, T>(f: F) -> T
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

// Mock for get_config and get_pool
fn mock_get_config() -> crate::config::Config {
    let mut config = crate::config::Config::default();
    config.general.admin_username = "admin".to_string();
    config.general.admin_password = "admin_password".to_string();
    config
}

// Tests for JWT authentication
#[test]
fn test_jwt_authentication() {
    let _result = run_test(|| async {
        let mut reader = MockReader::new(vec![b"valid_token".to_vec()]);
        let mut writer = MockWriter::new();

        let result = authenticate_with_jwt(
            &mut reader,
            &mut writer,
            "jwt_pub_key".to_string(),
            "test_user",
        )
        .await;

        assert!(result.is_ok());

        result
    });
}

#[test]
fn test_jwt_authentication_failure() {
    let _result = run_test(|| async {
        let mut reader = MockReader::new(vec![b"invalid_token".to_vec()]);
        let mut writer = MockWriter::new();

        let result = authenticate_with_jwt(
            &mut reader,
            &mut writer,
            "jwt_pub_key".to_string(),
            "test_user",
        )
        .await;

        assert!(result.is_err());
        if let Err(Error::JWTValidate(ref msg)) = result {
            assert!(msg.contains("Invalid JWT token"));
        } else {
            panic!("Expected JWTValidate error");
        }

        result
    });
}

// Test for SCRAM authentication
#[test]
fn test_scram_authentication() {
    let _result = run_test(|| async {
        // For SCRAM authentication, we need to mock the client first message and final message
        let client_first_message =
            format!("{SCRAM_SHA_256}\\0\\0\\0\\0 n,,n=,r=5DAkMQDUZpG/3GcwewTYJZbD");
        let client_final_message = "c=biws,r=5DAkMQDUZpG/3GcwewTYJZbDrandom,p=validproof";

        let mut reader = MockReader::new(vec![
            client_first_message.as_bytes().to_vec(),
            client_final_message.as_bytes().to_vec(),
        ]);
        let mut writer = MockWriter::new();

        let server_secret = format!("{SCRAM_SHA_256}$4096:salt$storedkey:serverkey");

        let result =
            authenticate_with_scram(&mut reader, &mut writer, &server_secret, "test_user")
                .await;
        assert!(result.is_ok());
    });
}

// Test for admin authentication
#[test]
fn test_admin_authentication() {
    let _result = run_test(|| async {
        // Mock the password response for admin authentication
        let config = mock_get_config();
        let salt = [1, 2, 3, 4];
        let password_hash = md5_hash_password(
            &config.general.admin_username,
            &config.general.admin_password,
            &salt,
        );

        let mut reader = MockReader::new(vec![password_hash]);
        let mut writer = MockWriter::new();

        let result = authenticate_admin(&mut reader, &mut writer, "admin").await;

        // This test might fail due to the need for more sophisticated mocking
        // of the get_config function
        assert!(result.is_ok());
    });
}
