use rand::Rng;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Fuzz client for sending malformed PostgreSQL protocol messages
/// All fuzz methods connect and authenticate FIRST, then send malformed data.
pub struct FuzzClient {
    addr: String,
}

impl FuzzClient {
    pub fn new(addr: &str) -> Self {
        Self {
            addr: addr.to_string(),
        }
    }

    /// Helper: connect and authenticate, return the stream ready for fuzz data
    async fn connect_and_auth(&self, user: &str, db: &str) -> tokio::io::Result<TcpStream> {
        let mut stream = TcpStream::connect(&self.addr).await?;

        // Startup message
        let mut startup = Vec::new();
        startup.extend_from_slice(&196608i32.to_be_bytes()); // protocol version 3.0
        startup.extend_from_slice(b"user\0");
        startup.extend_from_slice(user.as_bytes());
        startup.push(0);
        startup.extend_from_slice(b"database\0");
        startup.extend_from_slice(db.as_bytes());
        startup.push(0);
        startup.push(0);

        let len = (startup.len() + 4) as i32;
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&startup).await?;

        // Read until ReadyForQuery ('Z') to ensure authentication is complete
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                break; // Connection closed
            }
            // Look for ReadyForQuery message type 'Z' in the response
            if buf[..n].contains(&b'Z') {
                break;
            }
        }

        Ok(stream)
    }

    /// Broken header with invalid length (length=1, but minimum is 4)
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_broken_length_header(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        // Query message with length=1 (invalid, minimum is 4)
        let _ = stream.write_all(&[b'Q', 0, 0, 0, 1]).await;
        Ok(())
    }

    /// Negative length (-1)
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_negative_length(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        let _ = stream.write_all(&[b'Q', 0xFF, 0xFF, 0xFF, 0xFF]).await;
        Ok(())
    }

    /// Truncated message - claims length 100, sends only 3 bytes of data
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_truncated_message(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        let _ = stream
            .write_all(&[b'Q', 0, 0, 0, 100, b'S', b'E', b'L'])
            .await;
        Ok(())
    }

    /// Unknown message type
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_unknown_message_type(
        &self,
        user: &str,
        db: &str,
        msg_type: u8,
    ) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        let _ = stream.write_all(&[msg_type, 0, 0, 0, 4]).await;
        Ok(())
    }

    /// Server-only message type sent from client ('T' = RowDescription)
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_server_message_type(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        let _ = stream.write_all(&[b'T', 0, 0, 0, 6, 0, 0]).await;
        Ok(())
    }

    /// Null byte message type
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_null_message_type(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        let _ = stream.write_all(&[0x00, 0, 0, 0, 4]).await;
        Ok(())
    }

    /// Gigantic message length (256MB)
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_gigantic_length(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        let _ = stream.write_all(&[b'Q', 0x10, 0x00, 0x00, 0x00]).await;
        Ok(())
    }

    /// Random garbage data
    /// Connects and authenticates first, then sends malformed message.
    pub async fn send_random_garbage(
        &self,
        user: &str,
        db: &str,
        size: usize,
    ) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;
        let mut rng = rand::rng();
        let data: Vec<u8> = (0..size).map(|_| rng.random()).collect();
        let _ = stream.write_all(&data).await;
        Ok(())
    }

    /// Execute without Bind - protocol violation
    /// Connects and authenticates first, then sends protocol violation.
    pub async fn send_execute_without_bind(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;

        // Parse message
        let stmt_name = b"test_stmt\0";
        let query = b"SELECT 1\0";
        let parse_len = 4 + stmt_name.len() + query.len() + 2; // +2 for param count (i16)
        stream.write_all(b"P").await?;
        stream.write_all(&(parse_len as i32).to_be_bytes()).await?;
        stream.write_all(stmt_name).await?;
        stream.write_all(query).await?;
        stream.write_all(&0i16.to_be_bytes()).await?; // no params

        // Skip Bind, go directly to Execute - protocol violation!
        let portal = b"\0"; // unnamed portal
        let execute_len = 4 + portal.len() + 4; // +4 for max_rows
        stream.write_all(b"E").await?;
        stream
            .write_all(&(execute_len as i32).to_be_bytes())
            .await?;
        stream.write_all(portal).await?;
        stream.write_all(&0i32.to_be_bytes()).await?; // max_rows = 0 (all)

        // Sync
        stream.write_all(&[b'S', 0, 0, 0, 4]).await?;

        Ok(())
    }

    /// Bind to nonexistent statement - protocol violation
    /// Connects and authenticates first, then sends protocol violation.
    pub async fn send_bind_nonexistent(&self, user: &str, db: &str) -> tokio::io::Result<()> {
        let mut stream = self.connect_and_auth(user, db).await?;

        // Bind to nonexistent statement
        let portal = b"\0"; // unnamed portal
        let stmt_name = b"nonexistent_statement\0";
        let bind_len = 4 + portal.len() + stmt_name.len() + 2 + 2 + 2; // format codes + params + result formats
        stream.write_all(b"B").await?;
        stream.write_all(&(bind_len as i32).to_be_bytes()).await?;
        stream.write_all(portal).await?;
        stream.write_all(stmt_name).await?;
        stream.write_all(&0i16.to_be_bytes()).await?; // no format codes
        stream.write_all(&0i16.to_be_bytes()).await?; // no params
        stream.write_all(&0i16.to_be_bytes()).await?; // no result format codes

        // Sync
        stream.write_all(&[b'S', 0, 0, 0, 4]).await?;

        Ok(())
    }

    /// Multiple random malformed connections
    /// All attacks connect and authenticate first, then send malformed data.
    pub async fn attack_random(&self, user: &str, db: &str, count: usize) -> tokio::io::Result<()> {
        let mut rng = rand::rng();

        for _ in 0..count {
            let attack_type = rng.random_range(0..7);
            let result = match attack_type {
                0 => self.send_broken_length_header(user, db).await,
                1 => self.send_negative_length(user, db).await,
                2 => self.send_truncated_message(user, db).await,
                3 => self.send_unknown_message_type(user, db, b'X').await,
                4 => self.send_server_message_type(user, db).await,
                5 => self.send_null_message_type(user, db).await,
                6 => self.send_random_garbage(user, db, 100).await,
                _ => Ok(()),
            };
            // Ignore errors - we expect some connections to fail
            let _ = result;
        }

        Ok(())
    }
}
