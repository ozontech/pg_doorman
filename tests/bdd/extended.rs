use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{then, when};

// Helper function to format message details for debugging
fn format_message_details(msg_type: char, data: &[u8]) -> String {
    let mut details = format!("type='{}' len={}", msg_type, data.len());

    match msg_type {
        'R' => {
            // Authentication request
            if data.len() >= 4 {
                let auth_type = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                details.push_str(&format!(" [AuthenticationRequest type={}]", auth_type));
            }
        }
        'S' => {
            // ParameterStatus: name\0value\0
            if let Some(null_pos) = data.iter().position(|&b| b == 0) {
                let name = String::from_utf8_lossy(&data[..null_pos]);
                let value = String::from_utf8_lossy(
                    data[null_pos + 1..]
                        .split(|&b| b == 0)
                        .next()
                        .unwrap_or(&[]),
                );
                details.push_str(&format!(" [ParameterStatus {}={}]", name, value));
            }
        }
        'K' => {
            // BackendKeyData: process_id(4) + secret_key(4)
            if data.len() >= 8 {
                let pid = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let key = i32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                details.push_str(&format!(" [BackendKeyData pid={} key={}]", pid, key));
            }
        }
        'Z' => {
            // ReadyForQuery: status(1)
            if !data.is_empty() {
                let status = match data[0] {
                    b'I' => "Idle",
                    b'T' => "InTransaction",
                    b'E' => "FailedTransaction",
                    _ => "Unknown",
                };
                details.push_str(&format!(" [ReadyForQuery status={}]", status));
            }
        }
        'T' => {
            // RowDescription
            if data.len() >= 2 {
                let field_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [RowDescription fields={}]", field_count));
            }
        }
        'D' => {
            // DataRow
            if data.len() >= 2 {
                let field_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [DataRow fields={}]", field_count));
            }
        }
        'C' => {
            // CommandComplete: tag\0
            if let Some(null_pos) = data.iter().position(|&b| b == 0) {
                let tag = String::from_utf8_lossy(&data[..null_pos]);
                details.push_str(&format!(" [CommandComplete tag='{}']", tag));
            }
        }
        'E' => {
            // ErrorResponse: parse fields
            details.push_str(" [ErrorResponse");
            let mut pos = 0;
            while pos < data.len() {
                let field_type = data[pos] as char;
                if field_type == '\0' {
                    break;
                }
                pos += 1;
                if let Some(null_pos) = data[pos..].iter().position(|&b| b == 0) {
                    let value = String::from_utf8_lossy(&data[pos..pos + null_pos]);
                    match field_type {
                        'S' => details.push_str(&format!(" severity={}", value)),
                        'C' => details.push_str(&format!(" code={}", value)),
                        'M' => details.push_str(&format!(" message={}", value)),
                        _ => {}
                    }
                    pos += null_pos + 1;
                } else {
                    break;
                }
            }
            details.push(']');
        }
        'N' => {
            // NoticeResponse: similar to ErrorResponse
            details.push_str(" [NoticeResponse");
            let mut pos = 0;
            while pos < data.len() {
                let field_type = data[pos] as char;
                if field_type == '\0' {
                    break;
                }
                pos += 1;
                if let Some(null_pos) = data[pos..].iter().position(|&b| b == 0) {
                    let value = String::from_utf8_lossy(&data[pos..pos + null_pos]);
                    match field_type {
                        'S' => details.push_str(&format!(" severity={}", value)),
                        'C' => details.push_str(&format!(" code={}", value)),
                        'M' => details.push_str(&format!(" message={}", value)),
                        _ => {}
                    }
                    pos += null_pos + 1;
                } else {
                    break;
                }
            }
            details.push(']');
        }
        '1' => {
            // ParseComplete
            details.push_str(" [ParseComplete]");
        }
        '2' => {
            // BindComplete
            details.push_str(" [BindComplete]");
        }
        't' => {
            // ParameterDescription
            if data.len() >= 2 {
                let param_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [ParameterDescription params={}]", param_count));
            }
        }
        'n' => {
            // NoData
            details.push_str(" [NoData]");
        }
        's' => {
            // PortalSuspended
            details.push_str(" [PortalSuspended]");
        }
        _ => {
            // Unknown message type, show first 32 bytes as hex
            let preview_len = data.len().min(32);
            let hex_preview: String = data[..preview_len]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");
            details.push_str(&format!(
                " [data: {}{}]",
                hex_preview,
                if data.len() > 32 { "..." } else { "" }
            ));
        }
    }

    details
}

// BDD step implementations

#[when(
    regex = r#"^we login to postgres and pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn login_to_both(
    world: &mut DoormanWorld,
    user: String,
    password: String,
    database: String,
) {
    let pg_port = world.pg_port.expect("PostgreSQL not started");
    let doorman_port = world.doorman_port.expect("pg_doorman not started");

    let pg_addr = format!("127.0.0.1:{}", pg_port);
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    // Connect to PostgreSQL
    let mut pg_conn = PgConnection::connect(&pg_addr)
        .await
        .expect("Failed to connect to PostgreSQL");
    pg_conn
        .send_startup(&user, &database)
        .await
        .expect("Failed to send startup to PostgreSQL");
    pg_conn
        .authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to PostgreSQL");

    // Connect to pg_doorman
    let mut doorman_conn = PgConnection::connect(&doorman_addr)
        .await
        .expect("Failed to connect to pg_doorman");
    doorman_conn
        .send_startup(&user, &database)
        .await
        .expect("Failed to send startup to pg_doorman");
    doorman_conn
        .authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to pg_doorman");

    world.pg_conn = Some(pg_conn);
    world.doorman_conn = Some(doorman_conn);
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to both$"#)]
pub async fn send_simple_query_to_both(world: &mut DoormanWorld, query: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_simple_query(&query)
        .await
        .expect("Failed to send query to PostgreSQL");
    doorman_conn
        .send_simple_query(&query)
        .await
        .expect("Failed to send query to pg_doorman");

    // Read messages from both
    let pg_messages = pg_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from PostgreSQL");
    let doorman_messages = doorman_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from pg_doorman");

    world.pg_accumulated_messages.extend(pg_messages);
    world.doorman_accumulated_messages.extend(doorman_messages);
}

#[when(regex = r#"^we send Parse "([^"]*)" with query "([^"]+)" to both$"#)]
pub async fn send_parse_to_both(world: &mut DoormanWorld, name: String, query: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_parse(&name, &query)
        .await
        .expect("Failed to send Parse to PostgreSQL");
    doorman_conn
        .send_parse(&name, &query)
        .await
        .expect("Failed to send Parse to pg_doorman");
}

#[when(regex = r#"^we send Bind "([^"]*)" to "([^"]*)" with params "([^"]*)" to both$"#)]
pub async fn send_bind_to_both(
    world: &mut DoormanWorld,
    portal: String,
    statement: String,
    params_str: String,
) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    // Parse params - simple implementation for comma-separated values
    let params: Vec<Option<Vec<u8>>> = if params_str.is_empty() {
        vec![]
    } else {
        params_str
            .split(',')
            .map(|s| Some(s.trim().as_bytes().to_vec()))
            .collect()
    };

    pg_conn
        .send_bind(&portal, &statement, params.clone())
        .await
        .expect("Failed to send Bind to PostgreSQL");
    doorman_conn
        .send_bind(&portal, &statement, params)
        .await
        .expect("Failed to send Bind to pg_doorman");
}

#[when(regex = r#"^we send Describe "([^"])" "([^"]*)" to both$"#)]
pub async fn send_describe_to_both(world: &mut DoormanWorld, target_type: String, name: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    let target_char = target_type.chars().next().expect("Empty target type");

    pg_conn
        .send_describe(target_char, &name)
        .await
        .expect("Failed to send Describe to PostgreSQL");
    doorman_conn
        .send_describe(target_char, &name)
        .await
        .expect("Failed to send Describe to pg_doorman");
}

#[when(regex = r#"^we send Execute "([^"]*)" to both$"#)]
pub async fn send_execute_to_both(world: &mut DoormanWorld, portal: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_execute(&portal, 0)
        .await
        .expect("Failed to send Execute to PostgreSQL");
    doorman_conn
        .send_execute(&portal, 0)
        .await
        .expect("Failed to send Execute to pg_doorman");
}

/// Helper step to repeat a sequence of Parse, Bind, Describe, Execute messages multiple times
/// Format: we repeat <N> times: Parse "<name>" with query "<query>", Bind "<portal>" to "<statement>" with params "<params>", Describe "<type>" "<name>", Execute "<portal>" to both
#[allow(clippy::too_many_arguments)]
#[when(
    regex = r#"^we repeat (\d+) times: Parse "([^"]*)" with query "([^"]+)", Bind "([^"]*)" to "([^"]*)" with params "([^"]+)", Describe "([^"])" "([^"]*)", Execute "([^"]*)" to both$"#
)]
pub async fn repeat_extended_protocol_to_both(
    world: &mut DoormanWorld,
    times: usize,
    parse_name: String,
    query: String,
    bind_portal: String,
    bind_statement: String,
    params_str: String,
    describe_type: String,
    describe_name: String,
    execute_portal: String,
) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    // Parse params - simple implementation for comma-separated values
    let params: Vec<Option<Vec<u8>>> = params_str
        .split(',')
        .map(|s| Some(s.trim().as_bytes().to_vec()))
        .collect();

    let describe_char = describe_type.chars().next().expect("Empty describe type");

    // Send all messages N times
    for _ in 0..times {
        // Parse
        pg_conn
            .send_parse(&parse_name, &query)
            .await
            .expect("Failed to send Parse to PostgreSQL");
        doorman_conn
            .send_parse(&parse_name, &query)
            .await
            .expect("Failed to send Parse to pg_doorman");

        // Bind
        pg_conn
            .send_bind(&bind_portal, &bind_statement, params.clone())
            .await
            .expect("Failed to send Bind to PostgreSQL");
        doorman_conn
            .send_bind(&bind_portal, &bind_statement, params.clone())
            .await
            .expect("Failed to send Bind to pg_doorman");

        // Describe
        pg_conn
            .send_describe(describe_char, &describe_name)
            .await
            .expect("Failed to send Describe to PostgreSQL");
        doorman_conn
            .send_describe(describe_char, &describe_name)
            .await
            .expect("Failed to send Describe to pg_doorman");

        // Execute
        pg_conn
            .send_execute(&execute_portal, 0)
            .await
            .expect("Failed to send Execute to PostgreSQL");
        doorman_conn
            .send_execute(&execute_portal, 0)
            .await
            .expect("Failed to send Execute to pg_doorman");
    }

    // Send Sync to both
    pg_conn
        .send_sync()
        .await
        .expect("Failed to send Sync to PostgreSQL");
    doorman_conn
        .send_sync()
        .await
        .expect("Failed to send Sync to pg_doorman");

    // Read messages from both
    let pg_messages = pg_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from PostgreSQL");
    let doorman_messages = doorman_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from pg_doorman");

    world.pg_accumulated_messages.extend(pg_messages);
    world.doorman_accumulated_messages.extend(doorman_messages);
}

#[when(regex = r#"^we send Sync to both$"#)]
pub async fn send_sync_to_both(world: &mut DoormanWorld) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_sync()
        .await
        .expect("Failed to send Sync to PostgreSQL");
    doorman_conn
        .send_sync()
        .await
        .expect("Failed to send Sync to pg_doorman");

    // Read messages from both
    let pg_messages = pg_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from PostgreSQL");
    let doorman_messages = doorman_conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages from pg_doorman");

    world.pg_accumulated_messages.extend(pg_messages);
    world.doorman_accumulated_messages.extend(doorman_messages);
}

#[when(regex = r#"^we send Flush to both$"#)]
pub async fn send_flush_to_both(world: &mut DoormanWorld) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    pg_conn
        .send_flush()
        .await
        .expect("Failed to send Flush to PostgreSQL");
    doorman_conn
        .send_flush()
        .await
        .expect("Failed to send Flush to pg_doorman");
}

#[when(regex = r#"^we send Execute "([^"]*)" with max_rows "(\d+)" to both$"#)]
pub async fn send_execute_with_max_rows_to_both(
    world: &mut DoormanWorld,
    portal: String,
    max_rows: String,
) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    let max_rows_int: i32 = max_rows.parse().expect("Invalid max_rows value");

    pg_conn
        .send_execute(&portal, max_rows_int)
        .await
        .expect("Failed to send Execute to PostgreSQL");
    doorman_conn
        .send_execute(&portal, max_rows_int)
        .await
        .expect("Failed to send Execute to pg_doorman");
}

#[when(regex = r#"^we send Close "([^"])" "([^"]*)" to both$"#)]
pub async fn send_close_to_both(world: &mut DoormanWorld, target_type: String, name: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    let target_char = target_type.chars().next().expect("Empty target type");

    pg_conn
        .send_close(target_char, &name)
        .await
        .expect("Failed to send Close to PostgreSQL");
    doorman_conn
        .send_close(target_char, &name)
        .await
        .expect("Failed to send Close to pg_doorman");
}

#[when(regex = r#"^we verify partial response received from both$"#)]
pub async fn verify_partial_response(world: &mut DoormanWorld) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    // Read partial messages (without waiting for ReadyForQuery)
    let pg_messages = pg_conn
        .read_partial_messages()
        .await
        .expect("Failed to read partial messages from PostgreSQL");
    let doorman_messages = doorman_conn
        .read_partial_messages()
        .await
        .expect("Failed to read partial messages from pg_doorman");

    world.pg_accumulated_messages.extend(pg_messages);
    world.doorman_accumulated_messages.extend(doorman_messages);
}

/// Helper step to repeat a simple sequence of Parse, Bind, Execute messages multiple times
#[when(
    regex = r#"^we repeat (\d+) times: Parse "([^"]*)" with query "([^"]+)", Bind "([^"]*)" to "([^"]*)" with params "([^"]+)", Execute "([^"]*)" to both$"#
)]
#[allow(clippy::too_many_arguments)]
pub async fn repeat_simple_extended_protocol(
    world: &mut DoormanWorld,
    times: usize,
    parse_name: String,
    query: String,
    bind_portal: String,
    bind_statement: String,
    params_str: String,
    execute_portal: String,
) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    // Parse params - simple implementation for comma-separated values
    let params: Vec<Option<Vec<u8>>> = params_str
        .split(',')
        .map(|s| Some(s.trim().as_bytes().to_vec()))
        .collect();

    // Send all messages N times
    for _ in 0..times {
        // Parse
        pg_conn
            .send_parse(&parse_name, &query)
            .await
            .expect("Failed to send Parse to PostgreSQL");
        doorman_conn
            .send_parse(&parse_name, &query)
            .await
            .expect("Failed to send Parse to pg_doorman");

        // Bind
        pg_conn
            .send_bind(&bind_portal, &bind_statement, params.clone())
            .await
            .expect("Failed to send Bind to PostgreSQL");
        doorman_conn
            .send_bind(&bind_portal, &bind_statement, params.clone())
            .await
            .expect("Failed to send Bind to pg_doorman");

        // Execute
        pg_conn
            .send_execute(&execute_portal, 0)
            .await
            .expect("Failed to send Execute to PostgreSQL");
        doorman_conn
            .send_execute(&execute_portal, 0)
            .await
            .expect("Failed to send Execute to pg_doorman");
    }
}

/// Helper step to repeat a sequence with Close command
#[allow(clippy::too_many_arguments)]
#[when(
    regex = r#"^we repeat (\d+) times: Parse "([^"]*)" with query "([^"]+)", Bind "([^"]*)" to "([^"]*)" with params "([^"]+)", Describe "([^"])" "([^"]*)", Execute "([^"]*)", Close "([^"])" "([^"]*)" to both$"#
)]
pub async fn repeat_extended_protocol_with_close(
    world: &mut DoormanWorld,
    times: usize,
    parse_name: String,
    query: String,
    bind_portal: String,
    bind_statement: String,
    params_str: String,
    describe_type: String,
    describe_name: String,
    execute_portal: String,
    close_type: String,
    close_name: String,
) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    // Parse params - simple implementation for comma-separated values
    let params: Vec<Option<Vec<u8>>> = params_str
        .split(',')
        .map(|s| Some(s.trim().as_bytes().to_vec()))
        .collect();

    let describe_char = describe_type.chars().next().expect("Empty describe type");
    let close_char = close_type.chars().next().expect("Empty close type");

    // Send all messages N times
    for _ in 0..times {
        // Parse
        pg_conn
            .send_parse(&parse_name, &query)
            .await
            .expect("Failed to send Parse to PostgreSQL");
        doorman_conn
            .send_parse(&parse_name, &query)
            .await
            .expect("Failed to send Parse to pg_doorman");

        // Bind
        pg_conn
            .send_bind(&bind_portal, &bind_statement, params.clone())
            .await
            .expect("Failed to send Bind to PostgreSQL");
        doorman_conn
            .send_bind(&bind_portal, &bind_statement, params.clone())
            .await
            .expect("Failed to send Bind to pg_doorman");

        // Describe
        pg_conn
            .send_describe(describe_char, &describe_name)
            .await
            .expect("Failed to send Describe to PostgreSQL");
        doorman_conn
            .send_describe(describe_char, &describe_name)
            .await
            .expect("Failed to send Describe to pg_doorman");

        // Execute
        pg_conn
            .send_execute(&execute_portal, 0)
            .await
            .expect("Failed to send Execute to PostgreSQL");
        doorman_conn
            .send_execute(&execute_portal, 0)
            .await
            .expect("Failed to send Execute to pg_doorman");

        // Close
        pg_conn
            .send_close(close_char, &close_name)
            .await
            .expect("Failed to send Close to PostgreSQL");
        doorman_conn
            .send_close(close_char, &close_name)
            .await
            .expect("Failed to send Close to pg_doorman");
    }
}

#[then(regex = r#"^we should receive identical messages from both$"#)]
pub async fn verify_identical_messages(world: &mut DoormanWorld) {
    let pg_messages = &world.pg_accumulated_messages;
    let doorman_messages = &world.doorman_accumulated_messages;

    // Debug output with detailed message information
    if pg_messages.len() != doorman_messages.len() {
        eprintln!("\n=== MESSAGE COUNT MISMATCH ===");
        eprintln!("PostgreSQL: {} messages", pg_messages.len());
        eprintln!("pg_doorman: {} messages", doorman_messages.len());

        eprintln!("\n=== PostgreSQL messages ===");
        for (i, (msg_type, data)) in pg_messages.iter().enumerate() {
            eprintln!("  [{}] {}", i, format_message_details(*msg_type, data));
        }

        eprintln!("\n=== pg_doorman messages ===");
        for (i, (msg_type, data)) in doorman_messages.iter().enumerate() {
            eprintln!("  [{}] {}", i, format_message_details(*msg_type, data));
        }
        eprintln!();
    }

    assert_eq!(
        pg_messages.len(),
        doorman_messages.len(),
        "Number of messages differs: PostgreSQL={}, pg_doorman={}",
        pg_messages.len(),
        doorman_messages.len()
    );

    for (i, (pg_msg, doorman_msg)) in pg_messages.iter().zip(doorman_messages.iter()).enumerate() {
        let (pg_type, pg_data) = pg_msg;
        let (doorman_type, doorman_data) = doorman_msg;

        // Check message type
        if pg_type != doorman_type {
            eprintln!("\n=== MESSAGE TYPE MISMATCH at position {} ===", i);
            eprintln!("PostgreSQL: {}", format_message_details(*pg_type, pg_data));
            eprintln!(
                "pg_doorman: {}",
                format_message_details(*doorman_type, doorman_data)
            );
            panic!(
                "Message {} type differs: PostgreSQL='{}', pg_doorman='{}'",
                i, pg_type, doorman_type
            );
        }

        // Check message length
        if pg_data.len() != doorman_data.len() {
            eprintln!("\n=== MESSAGE LENGTH MISMATCH at position {} ===", i);
            eprintln!("PostgreSQL: {}", format_message_details(*pg_type, pg_data));
            eprintln!(
                "pg_doorman: {}",
                format_message_details(*doorman_type, doorman_data)
            );

            // Show hex diff for first 64 bytes
            let max_len = pg_data.len().max(doorman_data.len()).min(64);
            eprintln!("\n--- Hex comparison (first {} bytes) ---", max_len);
            eprintln!(
                "PostgreSQL: {}",
                pg_data
                    .iter()
                    .take(max_len)
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            eprintln!(
                "pg_doorman: {}",
                doorman_data
                    .iter()
                    .take(max_len)
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ")
            );

            panic!(
                "Message {} length differs: PostgreSQL={}, pg_doorman={}",
                i,
                pg_data.len(),
                doorman_data.len()
            );
        }

        // Check message data
        if pg_data != doorman_data {
            eprintln!("\n=== MESSAGE DATA MISMATCH at position {} ===", i);
            eprintln!("PostgreSQL: {}", format_message_details(*pg_type, pg_data));
            eprintln!(
                "pg_doorman: {}",
                format_message_details(*doorman_type, doorman_data)
            );

            // Find first difference
            for (pos, (pg_byte, doorman_byte)) in
                pg_data.iter().zip(doorman_data.iter()).enumerate()
            {
                if pg_byte != doorman_byte {
                    eprintln!(
                        "\nFirst difference at byte {}: PostgreSQL=0x{:02x} pg_doorman=0x{:02x}",
                        pos, pg_byte, doorman_byte
                    );

                    // Show context around the difference
                    let start = pos.saturating_sub(8);
                    let end = (pos + 8).min(pg_data.len());
                    eprintln!("Context (bytes {}-{}):", start, end);
                    eprintln!(
                        "  PostgreSQL: {}",
                        pg_data[start..end]
                            .iter()
                            .map(|b| format!("{:02x}", b))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    eprintln!(
                        "  pg_doorman: {}",
                        doorman_data[start..end]
                            .iter()
                            .map(|b| format!("{:02x}", b))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    break;
                }
            }

            panic!("Message {} data differs", i);
        }

        println!(
            "Message {} is identical: {}",
            i,
            format_message_details(*pg_type, pg_data)
        );
    }

    // Clear accumulated messages for next scenario
    world.pg_accumulated_messages.clear();
    world.doorman_accumulated_messages.clear();
}

// Steps for named sessions (reuse-server-backend tests)

#[when(
    regex = r#"^we create session "([^"]+)" to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn create_named_session(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    // Connect to pg_doorman
    let mut conn = PgConnection::connect(&doorman_addr)
        .await
        .expect("Failed to connect to pg_doorman");
    conn.send_startup(&user, &database)
        .await
        .expect("Failed to send startup to pg_doorman");
    conn.authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to pg_doorman");

    world.named_sessions.insert(session_name, conn);
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)"$"#)]
pub async fn send_simple_query_to_session(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read all messages until ReadyForQuery
    let _messages = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and store backend_pid$"#)]
pub async fn send_simple_query_and_store_backend_pid(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read messages and parse backend_pid
    let mut backend_pid: Option<i32> = None;
    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'T' => {
                // RowDescription - skip
            }
            'D' => {
                // DataRow - parse the integer value
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]);
                    if field_count == 1 {
                        // Read field length (4 bytes)
                        let field_len = i32::from_be_bytes([data[2], data[3], data[4], data[5]]);
                        if field_len > 0 {
                            // Read the value as string and parse to int
                            let value_bytes = &data[6..6 + field_len as usize];
                            let value_str = String::from_utf8_lossy(value_bytes);
                            backend_pid =
                                Some(value_str.parse().expect("Failed to parse backend_pid"));
                        }
                    }
                }
            }
            'C' => {
                // CommandComplete - skip
            }
            'Z' => {
                // ReadyForQuery - done
                break;
            }
            'E' => {
                // Error - this is expected for "bad sql"
                eprintln!(
                    "Error received (expected for bad sql): {:?}",
                    String::from_utf8_lossy(&data)
                );
                // Continue reading until ReadyForQuery
            }
            _ => {
                // Other messages - skip
            }
        }
    }

    if let Some(pid) = backend_pid {
        world.session_backend_pids.insert(session_name, pid);
    }
}

#[when(regex = r#"^we sleep (\d+)ms$"#)]
pub async fn sleep_ms(_world: &mut DoormanWorld, ms: String) {
    let duration = ms.parse::<u64>().expect("Invalid sleep duration");
    tokio::time::sleep(tokio::time::Duration::from_millis(duration)).await;
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and store backend_pid as "([^"]+)"$"#
)]
pub async fn send_simple_query_and_store_named_backend_pid(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
    pid_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read messages and parse backend_pid
    let mut backend_pid: Option<i32> = None;
    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'T' => {
                // RowDescription - skip
            }
            'D' => {
                // DataRow - parse the integer value
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]);
                    if field_count == 1 {
                        // Read field length (4 bytes)
                        let field_len = i32::from_be_bytes([data[2], data[3], data[4], data[5]]);
                        if field_len > 0 {
                            // Read the value as string and parse to int
                            let value_bytes = &data[6..6 + field_len as usize];
                            let value_str = String::from_utf8_lossy(value_bytes);
                            backend_pid =
                                Some(value_str.parse().expect("Failed to parse backend_pid"));
                        }
                    }
                }
            }
            'C' => {
                // CommandComplete - skip
            }
            'Z' => {
                // ReadyForQuery - done
                break;
            }
            'E' => {
                // Error - this is expected for "bad sql"
                eprintln!(
                    "Error received (expected for bad sql): {:?}",
                    String::from_utf8_lossy(&data)
                );
                // Continue reading until ReadyForQuery
            }
            _ => {
                // Other messages - skip
            }
        }
    }

    if let Some(pid) = backend_pid {
        world
            .named_backend_pids
            .insert((session_name, pid_name), pid);
    }
}

#[then(
    regex = r#"^backend_pid from session "([^"]+)" should equal backend_pid from session "([^"]+)"$"#
)]
pub async fn compare_backend_pids(world: &mut DoormanWorld, session1: String, session2: String) {
    let pid1 = world
        .session_backend_pids
        .get(&session1)
        .unwrap_or_else(|| panic!("Backend PID for session '{}' not found", session1));
    let pid2 = world
        .session_backend_pids
        .get(&session2)
        .unwrap_or_else(|| panic!("Backend PID for session '{}' not found", session2));

    println!("Session '{}' backend_pid: {}", session1, pid1);
    println!("Session '{}' backend_pid: {}", session2, pid2);

    assert_eq!(
        pid1, pid2,
        "Backend PIDs should be equal: session '{}'={}, session '{}'={}",
        session1, pid1, session2, pid2
    );
}

#[then(
    regex = r#"^backend_pid from session "([^"]+)" should not equal backend_pid from session "([^"]+)"$"#
)]
pub async fn compare_backend_pids_not_equal(
    world: &mut DoormanWorld,
    session1: String,
    session2: String,
) {
    let pid1 = world
        .session_backend_pids
        .get(&session1)
        .unwrap_or_else(|| panic!("Backend PID for session '{}' not found", session1));
    let pid2 = world
        .session_backend_pids
        .get(&session2)
        .unwrap_or_else(|| panic!("Backend PID for session '{}' not found", session2));

    println!("Session '{}' backend_pid: {}", session1, pid1);
    println!("Session '{}' backend_pid: {}", session2, pid2);

    assert_ne!(
        pid1, pid2,
        "Backend PIDs should NOT be equal: session '{}'={}, session '{}'={}",
        session1, pid1, session2, pid2
    );
}

#[then(
    regex = r#"^backend_pid "([^"]+)" from session "([^"]+)" should equal initial backend_pid from session "([^"]+)"$"#
)]
pub async fn compare_named_backend_pid_with_initial(
    world: &mut DoormanWorld,
    pid_name: String,
    session_name: String,
    initial_session_name: String,
) {
    let named_pid = world
        .named_backend_pids
        .get(&(session_name.clone(), pid_name.clone()))
        .unwrap_or_else(|| {
            panic!(
                "Named backend PID '{}' for session '{}' not found",
                pid_name, session_name
            )
        });
    let initial_pid = world
        .session_backend_pids
        .get(&initial_session_name)
        .unwrap_or_else(|| {
            panic!(
                "Initial backend PID for session '{}' not found",
                initial_session_name
            )
        });

    println!(
        "Session '{}' named backend_pid '{}': {}",
        session_name, pid_name, named_pid
    );
    println!(
        "Session '{}' initial backend_pid: {}",
        initial_session_name, initial_pid
    );

    assert_eq!(
        named_pid, initial_pid,
        "Named backend PID '{}' from session '{}' ({}) should equal initial backend PID from session '{}' ({})",
        pid_name, session_name, named_pid, initial_session_name, initial_pid
    );
}

// Steps for prepared statements cache tests

#[when(regex = r#"^we send Parse "([^"]*)" with query "([^"]+)" to session "([^"]+)"$"#)]
pub async fn send_parse_to_session(
    world: &mut DoormanWorld,
    name: String,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.send_parse(&name, &query)
        .await
        .expect("Failed to send Parse");
}

#[when(
    regex = r#"^we send Bind "([^"]*)" to "([^"]*)" with params "([^"]*)" to session "([^"]+)"$"#
)]
pub async fn send_bind_to_session(
    world: &mut DoormanWorld,
    portal: String,
    statement: String,
    params_str: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    // Parse params - simple implementation for comma-separated values
    let params: Vec<Option<Vec<u8>>> = if params_str.is_empty() {
        vec![]
    } else {
        params_str
            .split(',')
            .map(|s| Some(s.trim().as_bytes().to_vec()))
            .collect()
    };

    conn.send_bind(&portal, &statement, params)
        .await
        .expect("Failed to send Bind");
}

#[when(regex = r#"^we send Execute "([^"]*)" to session "([^"]+)"$"#)]
pub async fn send_execute_to_session(
    world: &mut DoormanWorld,
    portal: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.send_execute(&portal, 0)
        .await
        .expect("Failed to send Execute");
}

#[when(regex = r#"^we send Sync to session "([^"]+)"$"#)]
pub async fn send_sync_to_session(world: &mut DoormanWorld, session_name: String) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));

    conn.send_sync().await.expect("Failed to send Sync");
    
    // Read all messages until ReadyForQuery and store them
    let messages = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");
    
    world
        .session_messages
        .insert(session_name.clone(), messages);
}

#[when(regex = r#"^we close session "([^"]+)"$"#)]
pub async fn close_session(world: &mut DoormanWorld, session_name: String) {
    world
        .named_sessions
        .remove(&session_name)
        .unwrap_or_else(|| panic!("Session '{}' not found", session_name));
}

#[then(regex = r#"^session "([^"]+)" should receive DataRow with "([^"]+)"$"#)]
pub async fn session_should_receive_datarow(
    world: &mut DoormanWorld,
    session_name: String,
    expected_value: String,
) {
    // Get messages from the stored session messages
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{}'", session_name));

    // Find DataRow in the messages
    let mut found_value: Option<String> = None;
    for (msg_type, data) in messages {
        match msg_type {
            'D' => {
                // DataRow - parse the value
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]);
                    if field_count >= 1 {
                        // Read first field length (4 bytes)
                        let field_len = i32::from_be_bytes([data[2], data[3], data[4], data[5]]);
                        if field_len > 0 {
                            // Read the value as string
                            let value_bytes = &data[6..6 + field_len as usize];
                            let value_str = String::from_utf8_lossy(value_bytes).to_string();
                            found_value = Some(value_str);
                            break;
                        }
                    }
                }
            }
            'E' => {
                // Error
                panic!(
                    "Error received from session '{}': {:?}",
                    session_name,
                    String::from_utf8_lossy(data)
                );
            }
            _ => {
                // Other messages - skip
            }
        }
    }

    let actual_value = found_value.unwrap_or_else(|| {
        panic!(
            "No DataRow received from session '{}', expected '{}'",
            session_name, expected_value
        )
    });

    assert_eq!(
        actual_value, expected_value,
        "Session '{}': expected '{}', got '{}'",
        session_name, expected_value, actual_value
    );
}
