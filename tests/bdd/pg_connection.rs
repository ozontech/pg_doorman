use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct PgConnection {
    stream: TcpStream,
    /// Process ID from BackendKeyData (used for cancel requests)
    process_id: Option<i32>,
    /// Secret key from BackendKeyData (used for cancel requests)
    secret_key: Option<i32>,
}

impl PgConnection {
    pub async fn connect(addr: &str) -> tokio::io::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self {
            stream,
            process_id: None,
            secret_key: None,
        })
    }

    pub async fn send_startup(&mut self, user: &str, database: &str) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&196608i32.to_be_bytes()); // protocol version 3.0
        msg.extend_from_slice(b"user\0");
        msg.extend_from_slice(user.as_bytes());
        msg.push(0);
        msg.extend_from_slice(b"database\0");
        msg.extend_from_slice(database.as_bytes());
        msg.push(0);
        msg.push(0);

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn read_message(&mut self) -> tokio::io::Result<(char, Vec<u8>)> {
        let mut header = [0u8; 5];
        if let Err(e) = self.stream.read_exact(&mut header).await {
            eprintln!("Failed to read message header: {}", e);
            return Err(e);
        }
        let msg_type = header[0] as char;
        let len = i32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if len < 4 {
            panic!("Invalid message length: {}", len);
        }
        let mut data = vec![0u8; len - 4];
        self.stream.read_exact(&mut data).await?;
        Ok((msg_type, data))
    }

    pub async fn authenticate(&mut self, user: &str, password: &str) -> tokio::io::Result<()> {
        loop {
            let (msg_type, data) = self.read_message().await?;
            match msg_type {
                'R' => {
                    let auth_type = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                    match auth_type {
                        0 => {
                            // AuthenticationOk
                            continue;
                        }
                        3 => {
                            // AuthenticationCleartextPassword
                            self.send_password(password).await?;
                        }
                        5 => {
                            // AuthenticationMD5Password
                            let salt = &data[4..8];
                            let hash = self.compute_md5_hash(user, password, salt);
                            self.send_password(&hash).await?;
                        }
                        _ => panic!("Unsupported auth type: {}", auth_type),
                    }
                }
                'S' => continue, // ParameterStatus
                'K' => {
                    // BackendKeyData: process_id (4 bytes) + secret_key (4 bytes)
                    if data.len() >= 8 {
                        self.process_id =
                            Some(i32::from_be_bytes([data[0], data[1], data[2], data[3]]));
                        self.secret_key =
                            Some(i32::from_be_bytes([data[4], data[5], data[6], data[7]]));
                    }
                    continue;
                }
                'Z' => {
                    // ReadyForQuery
                    if data[0] == b'I' {
                        return Ok(());
                    }
                }
                'E' => {
                    panic!("Error during auth: {:?}", String::from_utf8_lossy(&data));
                }
                _ => {
                    println!("Received message during auth: {} {:?}", msg_type, data);
                }
            }
        }
    }

    async fn send_password(&mut self, password: &str) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.extend_from_slice(password.as_bytes());
        msg.push(0);

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'p');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    fn compute_md5_hash(&self, user: &str, password: &str, salt: &[u8]) -> String {
        use md5::Digest;
        let mut hasher = md5::Md5::new();
        hasher.update(password.as_bytes());
        hasher.update(user.as_bytes());
        let res1 = hasher.finalize();
        let hex1 = format!("{:x}", res1);

        let mut hasher = md5::Md5::new();
        hasher.update(hex1.as_bytes());
        hasher.update(salt);
        let res2 = hasher.finalize();
        format!("md5{:x}", res2)
    }

    pub async fn send_simple_query(&mut self, query: &str) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.extend_from_slice(query.as_bytes());
        msg.push(0);

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'Q');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn send_parse(&mut self, name: &str, query: &str) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.extend_from_slice(name.as_bytes());
        msg.push(0);
        msg.extend_from_slice(query.as_bytes());
        msg.push(0);
        msg.extend_from_slice(&0i16.to_be_bytes()); // number of parameter data types (0)

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'P');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn send_bind(
        &mut self,
        portal: &str,
        statement: &str,
        params: Vec<Option<Vec<u8>>>,
    ) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.extend_from_slice(portal.as_bytes());
        msg.push(0);
        msg.extend_from_slice(statement.as_bytes());
        msg.push(0);

        msg.extend_from_slice(&0i16.to_be_bytes()); // parameter format codes (0 for all strings)

        msg.extend_from_slice(&(params.len() as i16).to_be_bytes());
        for param in params {
            match param {
                Some(p) => {
                    msg.extend_from_slice(&(p.len() as i32).to_be_bytes());
                    msg.extend(p);
                }
                None => {
                    msg.extend_from_slice(&(-1i32).to_be_bytes());
                }
            }
        }

        msg.extend_from_slice(&0i16.to_be_bytes()); // result-column format codes (0 for all strings)

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'B');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn send_describe(&mut self, target_type: char, name: &str) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.push(target_type as u8);
        msg.extend_from_slice(name.as_bytes());
        msg.push(0);

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'D');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn send_execute(&mut self, portal: &str, max_rows: i32) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.extend_from_slice(portal.as_bytes());
        msg.push(0);
        msg.extend_from_slice(&max_rows.to_be_bytes());

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'E');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn send_sync(&mut self) -> tokio::io::Result<()> {
        let mut full_msg = Vec::new();
        full_msg.push(b'S');
        full_msg.extend_from_slice(&4i32.to_be_bytes());

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn send_flush(&mut self) -> tokio::io::Result<()> {
        let mut full_msg = Vec::new();
        full_msg.push(b'H');
        full_msg.extend_from_slice(&4i32.to_be_bytes());

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn send_close(&mut self, target_type: char, name: &str) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.push(target_type as u8);
        msg.extend_from_slice(name.as_bytes());
        msg.push(0);

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'C');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    /// Send CopyData message ('d') - used during COPY FROM STDIN
    pub async fn send_copy_data(&mut self, data: &[u8]) -> tokio::io::Result<()> {
        let len = (data.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'd');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend_from_slice(data);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    /// Send CopyDone message ('c') - signals end of COPY FROM STDIN data
    pub async fn send_copy_done(&mut self) -> tokio::io::Result<()> {
        let mut full_msg = Vec::new();
        full_msg.push(b'c');
        full_msg.extend_from_slice(&4i32.to_be_bytes());

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    /// Send CopyFail message ('f') - signals COPY FROM STDIN failure
    #[allow(dead_code)]
    pub async fn send_copy_fail(&mut self, error_message: &str) -> tokio::io::Result<()> {
        let mut msg = Vec::new();
        msg.extend_from_slice(error_message.as_bytes());
        msg.push(0);

        let len = (msg.len() + 4) as i32;
        let mut full_msg = Vec::new();
        full_msg.push(b'f');
        full_msg.extend_from_slice(&len.to_be_bytes());
        full_msg.extend(msg);

        self.stream.write_all(&full_msg).await?;
        Ok(())
    }

    pub async fn read_all_messages_until_ready(
        &mut self,
    ) -> tokio::io::Result<Vec<(char, Vec<u8>)>> {
        let mut messages = Vec::new();
        loop {
            let (msg_type, data) = self.read_message().await?;
            if msg_type == 'Z' {
                messages.push((msg_type, data));
                break;
            }
            // For simple query comparison, we might want to ignore ParameterStatus (S)
            // as they can be different or in different order
            if msg_type != 'S' && msg_type != 'K' {
                messages.push((msg_type, data));
            }
        }
        Ok(messages)
    }

    pub async fn read_partial_messages(&mut self) -> tokio::io::Result<Vec<(char, Vec<u8>)>> {
        let mut messages = Vec::new();
        // Read messages until we get at least one, but don't wait for ReadyForQuery
        loop {
            // Check if there's data available without blocking
            match tokio::time::timeout(std::time::Duration::from_millis(100), self.read_message())
                .await
            {
                Ok(Ok((msg_type, data))) => {
                    if msg_type == 'Z' {
                        messages.push((msg_type, data));
                        break;
                    }
                    if msg_type != 'S' && msg_type != 'K' {
                        messages.push((msg_type, data));
                    }
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    // Timeout - no more messages available
                    if !messages.is_empty() {
                        break;
                    }
                }
            }
        }
        Ok(messages)
    }

    /// Abruptly close the TCP connection (simulates network failure)
    pub async fn abort_connection(self) {
        // Drop the stream without proper shutdown - simulates network failure
        drop(self.stream);
    }

    /// Read a limited number of bytes from the stream (for partial read tests)
    /// Returns the number of bytes actually read
    pub async fn read_limited_bytes(&mut self, max_bytes: usize) -> tokio::io::Result<usize> {
        let mut total_read = 0;
        let mut buffer = vec![0u8; 8192]; // 8KB buffer

        while total_read < max_bytes {
            let to_read = std::cmp::min(buffer.len(), max_bytes - total_read);
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                self.stream.read(&mut buffer[..to_read]),
            )
            .await
            {
                Ok(Ok(0)) => break, // EOF
                Ok(Ok(n)) => {
                    total_read += n;
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => break, // Timeout - no more data available
            }
        }
        Ok(total_read)
    }

    /// Get the process ID from BackendKeyData (received during authentication)
    pub fn get_process_id(&self) -> Option<i32> {
        self.process_id
    }

    /// Get the secret key from BackendKeyData (received during authentication)
    pub fn get_secret_key(&self) -> Option<i32> {
        self.secret_key
    }

    /// Send a CancelRequest to the server
    /// This creates a new connection, sends the cancel request, and closes it
    /// Protocol: 16 bytes total - length (4) + cancel code (4) + process_id (4) + secret_key (4)
    pub async fn send_cancel_request(
        addr: &str,
        process_id: i32,
        secret_key: i32,
    ) -> tokio::io::Result<()> {
        let mut stream = TcpStream::connect(addr).await?;

        let mut msg = Vec::new();
        msg.extend_from_slice(&16i32.to_be_bytes()); // length = 16
        msg.extend_from_slice(&80877102i32.to_be_bytes()); // CancelRequest code
        msg.extend_from_slice(&process_id.to_be_bytes());
        msg.extend_from_slice(&secret_key.to_be_bytes());

        stream.write_all(&msg).await?;
        // Server will close the connection after receiving cancel request
        Ok(())
    }
}
