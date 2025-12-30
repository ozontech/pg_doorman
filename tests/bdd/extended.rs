use crate::world::DoormanWorld;
use bytes::{Buf, BytesMut};
use cucumber::{then, when};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct PgConnection {
    stream: TcpStream,
    buffer: BytesMut,
}

impl PgConnection {
    pub async fn connect(addr: &str) -> tokio::io::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self {
            stream,
            buffer: BytesMut::with_capacity(8192),
        })
    }

    pub async fn send_startup(
        &mut self,
        addr: &str,
        user: &str,
        database: &str,
    ) -> tokio::io::Result<()> {
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

        // println!("Sending startup message to {}", addr);
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
                'K' => continue, // BackendKeyData
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
        use md5::{Digest, Md5};
        let mut hasher = Md5::new();
        hasher.update(password.as_bytes());
        hasher.update(user.as_bytes());
        let res1 = hasher.finalize();
        let hex1 = format!("{:x}", res1);

        let mut hasher = Md5::new();
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

    pub async fn read_all_messages_until_ready(
        &mut self,
    ) -> tokio::io::Result<Vec<(char, Vec<u8>)>> {
        let mut messages = Vec::new();
        loop {
            let (msg_type, data) = self.read_message().await?;
            // println!("Read message: {} len: {}", msg_type, data.len() + 4);
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
}

#[when(
    expr = "we login to postgres and pg_doorman as {string} with password {string} and database {string}"
)]
async fn login_to_both(world: &mut DoormanWorld, user: String, password: String, database: String) {
    let pg_port = world.pg_port.expect("PostgreSQL port not set");
    let doorman_port = world.doorman_port.expect("pg_doorman port not set");

    // Give some time
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut pg_conn = PgConnection::connect(&format!("127.0.0.1:{}", pg_port))
        .await
        .expect("Failed to connect to PG");
    pg_conn
        .send_startup(&format!("127.0.0.1:{}", pg_port), &user, &database)
        .await
        .expect("Failed to send startup to PG");
    pg_conn
        .authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to PG");

    let mut doorman_conn = PgConnection::connect(&format!("127.0.0.1:{}", doorman_port))
        .await
        .expect("Failed to connect to Doorman");
    doorman_conn
        .send_startup(&format!("127.0.0.1:{}", doorman_port), &user, &database)
        .await
        .expect("Failed to send startup to Doorman");
    doorman_conn
        .authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to Doorman");

    // Store connections in world for next steps
    world.pg_conn = Some(pg_conn);
    world.doorman_conn = Some(doorman_conn);
}

#[when(expr = "we send SimpleQuery {string} to both")]
async fn send_query_to_both(world: &mut DoormanWorld, query: String) {
    let pg_conn = world.pg_conn.as_mut().expect("No PG connection");
    let doorman_conn = world.doorman_conn.as_mut().expect("No Doorman connection");

    pg_conn.send_simple_query(&query).await.expect("Failed to send query to PG");
    doorman_conn.send_simple_query(&query).await.expect("Failed to send query to Doorman");
}

#[when(expr = "we send Parse {string} with query {string} to both")]
async fn send_parse_to_both(world: &mut DoormanWorld, name: String, query: String) {
    let pg_conn = world.pg_conn.as_mut().expect("No PG connection");
    let doorman_conn = world.doorman_conn.as_mut().expect("No Doorman connection");

    pg_conn.send_parse(&name, &query).await.expect("Failed to send Parse to PG");
    doorman_conn.send_parse(&name, &query).await.expect("Failed to send Parse to Doorman");
}

#[when(expr = "we send Bind {string} to {string} with params {string} to both")]
async fn send_bind_to_both(world: &mut DoormanWorld, portal: String, statement: String, params_str: String) {
    let pg_conn = world.pg_conn.as_mut().expect("No PG connection");
    let doorman_conn = world.doorman_conn.as_mut().expect("No Doorman connection");

    // Very simple params parser for now: comma separated strings
    let params: Vec<Option<Vec<u8>>> = if params_str.is_empty() {
        vec![]
    } else {
        params_str.split(',').map(|s| Some(s.as_bytes().to_vec())).collect()
    };

    pg_conn.send_bind(&portal, &statement, params.clone()).await.expect("Failed to send Bind to PG");
    doorman_conn.send_bind(&portal, &statement, params).await.expect("Failed to send Bind to Doorman");
}

#[when(expr = "we send Describe {string} {string} to both")]
async fn send_describe_to_both(world: &mut DoormanWorld, target_type_str: String, name: String) {
    let pg_conn = world.pg_conn.as_mut().expect("No PG connection");
    let doorman_conn = world.doorman_conn.as_mut().expect("No Doorman connection");

    let target_type = target_type_str.chars().next().expect("Empty target type");

    pg_conn.send_describe(target_type, &name).await.expect("Failed to send Describe to PG");
    doorman_conn.send_describe(target_type, &name).await.expect("Failed to send Describe to Doorman");
}

#[when(expr = "we send Execute {string} to both")]
async fn send_execute_to_both(world: &mut DoormanWorld, portal: String) {
    let pg_conn = world.pg_conn.as_mut().expect("No PG connection");
    let doorman_conn = world.doorman_conn.as_mut().expect("No Doorman connection");

    pg_conn.send_execute(&portal, 0).await.expect("Failed to send Execute to PG");
    doorman_conn.send_execute(&portal, 0).await.expect("Failed to send Execute to Doorman");
}

#[when("we send Sync to both")]
async fn send_sync_to_both(world: &mut DoormanWorld) {
    let pg_conn = world.pg_conn.as_mut().expect("No PG connection");
    let doorman_conn = world.doorman_conn.as_mut().expect("No Doorman connection");

    pg_conn.send_sync().await.expect("Failed to send Sync to PG");
    doorman_conn.send_sync().await.expect("Failed to send Sync to Doorman");
}

#[then("we should receive identical messages from both")]
async fn receive_identical_messages(world: &mut DoormanWorld) {
    let pg_conn = world.pg_conn.as_mut().expect("No PG connection");
    let doorman_conn = world.doorman_conn.as_mut().expect("No Doorman connection");

    let pg_messages = pg_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read from PG");
    let doorman_messages = doorman_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read from Doorman");

    assert_eq!(
        pg_messages.len(),
        doorman_messages.len(),
        "Message count mismatch"
    );
    for (i, (pg_msg, doorman_msg)) in pg_messages.iter().zip(doorman_messages.iter()).enumerate() {
        assert_eq!(
            pg_msg.0, doorman_msg.0,
            "Message type mismatch at index {}",
            i
        );
        // We might want to be careful with some messages that might differ (e.g. RowDescription names if we use different PG versions, but here it should be same PG)
        // Some messages like CommandComplete might have slightly different tags if pg_doorman modifies them?
        // But the requirement says "одинаковые messages".

        // Skip comparing some fields if necessary, but let's try strict comparison first.
        // Actually, for 'C' (CommandComplete) and 'D' (DataRow) they should be identical.
        // For 'T' (RowDescription) they should also be identical.

        if pg_msg.0 == 'S' {
            // ParameterStatus might differ in order or set.
            // But usually they come after login.
            // Since we read UNTIL ReadyForQuery, we might see them if they are sent.
            continue;
        }

        assert_eq!(
            pg_msg.1, doorman_msg.1,
            "Message data mismatch for type {} at index {}",
            pg_msg.0, i
        );
    }
}
