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
                details.push_str(&format!(" [AuthenticationRequest type={auth_type}]"));
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
                details.push_str(&format!(" [ParameterStatus {name}={value}]"));
            }
        }
        'K' => {
            // BackendKeyData: process_id(4) + secret_key(4)
            if data.len() >= 8 {
                let pid = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let key = i32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                details.push_str(&format!(" [BackendKeyData pid={pid} key={key}]"));
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
                details.push_str(&format!(" [ReadyForQuery status={status}]"));
            }
        }
        'T' => {
            // RowDescription
            if data.len() >= 2 {
                let field_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [RowDescription fields={field_count}]"));
            }
        }
        'D' => {
            // DataRow
            if data.len() >= 2 {
                let field_count = i16::from_be_bytes([data[0], data[1]]);
                details.push_str(&format!(" [DataRow fields={field_count}]"));
            }
        }
        'C' => {
            // CommandComplete: tag\0
            if let Some(null_pos) = data.iter().position(|&b| b == 0) {
                let tag = String::from_utf8_lossy(&data[..null_pos]);
                details.push_str(&format!(" [CommandComplete tag='{tag}']"));
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
                        'S' => details.push_str(&format!(" severity={value}")),
                        'C' => details.push_str(&format!(" code={value}")),
                        'M' => details.push_str(&format!(" message={value}")),
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
                        'S' => details.push_str(&format!(" severity={value}")),
                        'C' => details.push_str(&format!(" code={value}")),
                        'M' => details.push_str(&format!(" message={value}")),
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
                details.push_str(&format!(" [ParameterDescription params={param_count}]"));
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
                .map(|b| format!("{b:02x}"))
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

/// Normalize RowDescription message by zeroing out table OIDs
/// RowDescription format:
///   Int16 - number of fields
///   For each field:
///     String - field name (null-terminated)
///     Int32 - table OID (if from a table, else 0) <- we zero this
///     Int16 - column attribute number
///     Int32 - data type OID
///     Int16 - data type size
///     Int32 - type modifier
///     Int16 - format code
fn normalize_row_description(data: &[u8]) -> Vec<u8> {
    let mut result = data.to_vec();
    if data.len() < 2 {
        return result;
    }

    let field_count = i16::from_be_bytes([data[0], data[1]]) as usize;
    let mut pos = 2;

    for _ in 0..field_count {
        // Skip field name (null-terminated string)
        while pos < result.len() && result[pos] != 0 {
            pos += 1;
        }
        pos += 1; // skip null terminator

        // Zero out table OID (4 bytes)
        if pos + 4 <= result.len() {
            result[pos] = 0;
            result[pos + 1] = 0;
            result[pos + 2] = 0;
            result[pos + 3] = 0;
        }
        pos += 4; // table OID

        pos += 2; // column attribute number
        pos += 4; // data type OID
        pos += 2; // data type size
        pos += 4; // type modifier
        pos += 2; // format code
    }

    result
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

    let pg_addr = format!("127.0.0.1:{pg_port}");
    let doorman_addr = format!("127.0.0.1:{doorman_port}");

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

    // Save credentials for reconnection
    world.last_user = Some(user);
    world.last_password = Some(password);
    world.last_database = Some(database);
}

#[when(regex = r#"^we disconnect from both$"#)]
pub async fn disconnect_from_both(world: &mut DoormanWorld) {
    // Drop connections by setting them to None
    // This will close the TCP connections gracefully
    world.pg_conn = None;
    world.doorman_conn = None;

    // Clear accumulated messages for fresh start after reconnect
    world.pg_accumulated_messages.clear();
    world.doorman_accumulated_messages.clear();
}

#[when(regex = r#"^we reconnect to both$"#)]
pub async fn reconnect_to_both(world: &mut DoormanWorld) {
    let user = world
        .last_user
        .clone()
        .expect("No previous login credentials - call login first");
    let password = world
        .last_password
        .clone()
        .expect("No previous login credentials - call login first");
    let database = world
        .last_database
        .clone()
        .expect("No previous login credentials - call login first");

    let pg_port = world.pg_port.expect("PostgreSQL not started");
    let doorman_port = world.doorman_port.expect("pg_doorman not started");

    let pg_addr = format!("127.0.0.1:{pg_port}");
    let doorman_addr = format!("127.0.0.1:{doorman_port}");

    // Connect to PostgreSQL
    let mut pg_conn = PgConnection::connect(&pg_addr)
        .await
        .expect("Failed to reconnect to PostgreSQL");
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
        .expect("Failed to reconnect to pg_doorman");
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

    // Clear accumulated messages for fresh start
    world.pg_accumulated_messages.clear();
    world.doorman_accumulated_messages.clear();
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

#[when(regex = r#"^we send CopyFromStdin "([^"]+)" with data "([^"]*)" to both$"#)]
pub async fn send_copy_from_stdin_to_both(world: &mut DoormanWorld, query: String, data: String) {
    let pg_conn = world.pg_conn.as_mut().expect("Not connected to PostgreSQL");
    let doorman_conn = world
        .doorman_conn
        .as_mut()
        .expect("Not connected to pg_doorman");

    // Unescape the data string (handle \t and \n)
    let unescaped_data = data.replace("\\t", "\t").replace("\\n", "\n");

    // Send the COPY command via simple query to PostgreSQL
    pg_conn
        .send_simple_query(&query)
        .await
        .expect("Failed to send COPY query to PostgreSQL");

    // Send the COPY command via simple query to pg_doorman
    doorman_conn
        .send_simple_query(&query)
        .await
        .expect("Failed to send COPY query to pg_doorman");

    // Read initial response from PostgreSQL (should be CopyInResponse 'G' or ErrorResponse 'E')
    let (pg_msg_type, pg_msg_data) = pg_conn
        .read_message()
        .await
        .expect("Failed to read COPY response from PostgreSQL");

    // Read initial response from pg_doorman
    let (doorman_msg_type, doorman_msg_data) = doorman_conn
        .read_message()
        .await
        .expect("Failed to read COPY response from pg_doorman");

    // If we got CopyInResponse ('G'), send the data and CopyDone
    if pg_msg_type == 'G' {
        // Send copy data to PostgreSQL
        if !unescaped_data.is_empty() {
            pg_conn
                .send_copy_data(unescaped_data.as_bytes())
                .await
                .expect("Failed to send CopyData to PostgreSQL");
        }
        pg_conn
            .send_copy_done()
            .await
            .expect("Failed to send CopyDone to PostgreSQL");
    } else {
        // Error response - store it
        world
            .pg_accumulated_messages
            .push((pg_msg_type, pg_msg_data.clone()));
    }

    if doorman_msg_type == 'G' {
        // Send copy data to pg_doorman
        if !unescaped_data.is_empty() {
            doorman_conn
                .send_copy_data(unescaped_data.as_bytes())
                .await
                .expect("Failed to send CopyData to pg_doorman");
        }
        doorman_conn
            .send_copy_done()
            .await
            .expect("Failed to send CopyDone to pg_doorman");
    } else {
        // Error response - store it
        world
            .doorman_accumulated_messages
            .push((doorman_msg_type, doorman_msg_data.clone()));
    }

    // Read remaining messages until ReadyForQuery from both
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

    // Check message count and provide detailed error if mismatch
    if pg_messages.len() != doorman_messages.len() {
        let mut error_msg = format!(
            "\n=== MESSAGE COUNT MISMATCH ===\nPostgreSQL: {} messages\npg_doorman: {} messages\n",
            pg_messages.len(),
            doorman_messages.len()
        );

        error_msg.push_str("\n=== PostgreSQL messages ===\n");
        for (i, (msg_type, data)) in pg_messages.iter().enumerate() {
            error_msg.push_str(&format!(
                "  [{}] {}\n",
                i,
                format_message_details(*msg_type, data)
            ));
        }

        error_msg.push_str("\n=== pg_doorman messages ===\n");
        for (i, (msg_type, data)) in doorman_messages.iter().enumerate() {
            error_msg.push_str(&format!(
                "  [{}] {}\n",
                i,
                format_message_details(*msg_type, data)
            ));
        }

        panic!(
            "Number of messages differs: PostgreSQL={}, pg_doorman={}\n{}",
            pg_messages.len(),
            doorman_messages.len(),
            error_msg
        );
    }

    for (i, (pg_msg, doorman_msg)) in pg_messages.iter().zip(doorman_messages.iter()).enumerate() {
        let (pg_type, pg_data) = pg_msg;
        let (doorman_type, doorman_data) = doorman_msg;

        // Check message type
        if pg_type != doorman_type {
            eprintln!("\n=== MESSAGE TYPE MISMATCH at position {i} ===");
            eprintln!("PostgreSQL: {}", format_message_details(*pg_type, pg_data));
            eprintln!(
                "pg_doorman: {}",
                format_message_details(*doorman_type, doorman_data)
            );
            panic!("Message {i} type differs: PostgreSQL='{pg_type}', pg_doorman='{doorman_type}'");
        }

        // Check message length
        if pg_data.len() != doorman_data.len() {
            eprintln!("\n=== MESSAGE LENGTH MISMATCH at position {i} ===");
            eprintln!("PostgreSQL: {}", format_message_details(*pg_type, pg_data));
            eprintln!(
                "pg_doorman: {}",
                format_message_details(*doorman_type, doorman_data)
            );

            // Show hex diff for first 64 bytes
            let max_len = pg_data.len().max(doorman_data.len()).min(64);
            eprintln!("\n--- Hex comparison (first {max_len} bytes) ---");
            eprintln!(
                "PostgreSQL: {}",
                pg_data
                    .iter()
                    .take(max_len)
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            eprintln!(
                "pg_doorman: {}",
                doorman_data
                    .iter()
                    .take(max_len)
                    .map(|b| format!("{b:02x}"))
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
        // For RowDescription ('T'), normalize table OIDs before comparison
        // because temp tables have different OIDs on different connections
        let (pg_data_normalized, doorman_data_normalized) = if *pg_type == 'T' {
            (
                normalize_row_description(pg_data),
                normalize_row_description(doorman_data),
            )
        } else {
            (pg_data.clone(), doorman_data.clone())
        };

        if pg_data_normalized != doorman_data_normalized {
            eprintln!("\n=== MESSAGE DATA MISMATCH at position {i} ===");
            eprintln!("PostgreSQL: {}", format_message_details(*pg_type, pg_data));
            eprintln!(
                "pg_doorman: {}",
                format_message_details(*doorman_type, doorman_data)
            );

            // Find first difference
            for (pos, (pg_byte, doorman_byte)) in pg_data_normalized
                .iter()
                .zip(doorman_data_normalized.iter())
                .enumerate()
            {
                if pg_byte != doorman_byte {
                    eprintln!(
                        "\nFirst difference at byte {pos}: PostgreSQL=0x{pg_byte:02x} pg_doorman=0x{doorman_byte:02x}"
                    );

                    // Show context around the difference
                    let start = pos.saturating_sub(8);
                    let end = (pos + 8).min(pg_data.len());
                    eprintln!("Context (bytes {start}-{end}):");
                    eprintln!(
                        "  PostgreSQL: {}",
                        pg_data[start..end]
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    eprintln!(
                        "  pg_doorman: {}",
                        doorman_data[start..end]
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    break;
                }
            }

            panic!("Message {i} data differs");
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
    let doorman_addr = format!("127.0.0.1:{doorman_port}");

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
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

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
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

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
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

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
        .unwrap_or_else(|| panic!("Backend PID for session '{session1}' not found"));
    let pid2 = world
        .session_backend_pids
        .get(&session2)
        .unwrap_or_else(|| panic!("Backend PID for session '{session2}' not found"));

    println!("Session '{session1}' backend_pid: {pid1}");
    println!("Session '{session2}' backend_pid: {pid2}");

    assert_eq!(
        pid1, pid2,
        "Backend PIDs should be equal: session '{session1}'={pid1}, session '{session2}'={pid2}"
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
        .unwrap_or_else(|| panic!("Backend PID for session '{session1}' not found"));
    let pid2 = world
        .session_backend_pids
        .get(&session2)
        .unwrap_or_else(|| panic!("Backend PID for session '{session2}' not found"));

    println!("Session '{session1}' backend_pid: {pid1}");
    println!("Session '{session2}' backend_pid: {pid2}");

    assert_ne!(
        pid1, pid2,
        "Backend PIDs should NOT be equal: session '{session1}'={pid1}, session '{session2}'={pid2}"
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
            panic!("Named backend PID '{pid_name}' for session '{session_name}' not found")
        });
    let initial_pid = world
        .session_backend_pids
        .get(&initial_session_name)
        .unwrap_or_else(|| {
            panic!("Initial backend PID for session '{initial_session_name}' not found")
        });

    println!("Session '{session_name}' named backend_pid '{pid_name}': {named_pid}");
    println!("Session '{initial_session_name}' initial backend_pid: {initial_pid}");

    assert_eq!(
        named_pid, initial_pid,
        "Named backend PID '{pid_name}' from session '{session_name}' ({named_pid}) should equal initial backend PID from session '{initial_session_name}' ({initial_pid})"
    );
}

#[then(
    regex = r#"^named backend_pid "([^"]+)" from session "([^"]+)" is different from "([^"]+)"$"#
)]
pub async fn compare_named_backend_pids_different(
    world: &mut DoormanWorld,
    pid_name1: String,
    session_name: String,
    pid_name2: String,
) {
    let pid1 = world
        .named_backend_pids
        .get(&(session_name.clone(), pid_name1.clone()))
        .unwrap_or_else(|| {
            panic!("Named backend PID '{pid_name1}' for session '{session_name}' not found")
        });
    let pid2 = world
        .named_backend_pids
        .get(&(session_name.clone(), pid_name2.clone()))
        .unwrap_or_else(|| {
            panic!("Named backend PID '{pid_name2}' for session '{session_name}' not found")
        });

    println!("Session '{session_name}' backend_pid '{pid_name1}': {pid1}");
    println!("Session '{session_name}' backend_pid '{pid_name2}': {pid2}");

    assert_ne!(
        pid1, pid2,
        "Backend PIDs should be different: '{pid_name1}' ({pid1}) vs '{pid_name2}' ({pid2})"
    );
}

#[when(regex = r#"^we terminate backend of session "([^"]+)" via session "([^"]+)"$"#)]
pub async fn terminate_backend_of_session(
    world: &mut DoormanWorld,
    target_session: String,
    killer_session: String,
) {
    // Get backend_pid of target session
    let backend_pid = world
        .session_backend_pids
        .get(&target_session)
        .unwrap_or_else(|| panic!("Backend PID for session '{target_session}' not found"));

    let terminate_query = format!("SELECT pg_terminate_backend({backend_pid})");
    eprintln!(
        "Terminating backend of session '{target_session}' (pid={backend_pid}) via session '{killer_session}'"
    );

    // Get killer session connection
    let conn = world
        .named_sessions
        .get_mut(&killer_session)
        .unwrap_or_else(|| panic!("Session '{killer_session}' not found"));

    // Send terminate query
    conn.send_simple_query(&terminate_query)
        .await
        .expect("Failed to send pg_terminate_backend query");

    // Read response
    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");
        match msg_type {
            'Z' => break, // ReadyForQuery - done
            'D' => {
                // DataRow - check result (should be 't' for true)
                eprintln!(
                    "pg_terminate_backend result: {:?}",
                    String::from_utf8_lossy(&data)
                );
            }
            'E' => {
                // Error
                panic!(
                    "Error executing pg_terminate_backend: {:?}",
                    String::from_utf8_lossy(&data)
                );
            }
            _ => {} // Other messages - skip
        }
    }
}

#[when(regex = r#"^we terminate backend "([^"]+)" from session "([^"]+)" via session "([^"]+)"$"#)]
pub async fn terminate_named_backend_via_session(
    world: &mut DoormanWorld,
    pid_name: String,
    source_session: String,
    killer_session: String,
) {
    // Get named backend_pid
    let backend_pid = world
        .named_backend_pids
        .get(&(source_session.clone(), pid_name.clone()))
        .unwrap_or_else(|| {
            panic!("Named backend PID '{pid_name}' from session '{source_session}' not found")
        });

    let terminate_query = format!("SELECT pg_terminate_backend({backend_pid})");
    eprintln!(
        "Terminating named backend '{pid_name}' from session '{source_session}' (pid={backend_pid}) via session '{killer_session}'"
    );

    // Get killer session connection
    let conn = world
        .named_sessions
        .get_mut(&killer_session)
        .unwrap_or_else(|| panic!("Session '{killer_session}' not found"));

    // Send terminate query
    conn.send_simple_query(&terminate_query)
        .await
        .expect("Failed to send pg_terminate_backend query");

    // Read response
    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");
        match msg_type {
            'Z' => break, // ReadyForQuery - done
            'D' => {
                // DataRow - check result (should be 't' for true)
                eprintln!(
                    "pg_terminate_backend result: {:?}",
                    String::from_utf8_lossy(&data)
                );
            }
            'E' => {
                // Error
                panic!(
                    "Error executing pg_terminate_backend: {:?}",
                    String::from_utf8_lossy(&data)
                );
            }
            _ => {} // Other messages - skip
        }
    }
}

// Steps for prepared statements cache tests

#[when(regex = r#"^we send Parse "([^"]*)" with query "([^"]+)" to session "([^"]+)"$"#)]
#[then(regex = r#"^we send Parse "([^"]*)" with query "([^"]+)" to session "([^"]+)"$"#)]
pub async fn send_parse_to_session(
    world: &mut DoormanWorld,
    name: String,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_parse(&name, &query)
        .await
        .expect("Failed to send Parse");
}

#[when(
    regex = r#"^we send Bind "([^"]*)" to "([^"]*)" with params "([^"]*)" to session "([^"]+)"$"#
)]
#[then(
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
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

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
#[then(regex = r#"^we send Execute "([^"]*)" to session "([^"]+)"$"#)]
pub async fn send_execute_to_session(
    world: &mut DoormanWorld,
    portal: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_execute(&portal, 0)
        .await
        .expect("Failed to send Execute");
}

#[when(regex = r#"^we send Sync to session "([^"]+)"$"#)]
#[then(regex = r#"^we send Sync to session "([^"]+)"$"#)]
pub async fn send_sync_to_session(world: &mut DoormanWorld, session_name: String) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

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
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));
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
        .unwrap_or_else(|| panic!("No messages stored for session '{session_name}'"));

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
        panic!("No DataRow received from session '{session_name}', expected '{expected_value}'")
    });

    assert_eq!(
        actual_value, expected_value,
        "Session '{session_name}': expected '{expected_value}', got '{actual_value}'"
    );
}

#[then(regex = r#"^we remember backend_pid from session "([^"]+)" as "([^"]+)"$"#)]
pub async fn remember_backend_pid_from_session(
    world: &mut DoormanWorld,
    session_name: String,
    pid_name: String,
) {
    // Get messages from the stored session messages
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{session_name}'"));

    // Find DataRow and extract backend_pid
    let mut backend_pid: Option<i32> = None;
    for (msg_type, data) in messages {
        match msg_type {
            'D' => {
                // DataRow - parse the integer value
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]);
                    if field_count >= 1 {
                        // Read first field length (4 bytes)
                        let field_len = i32::from_be_bytes([data[2], data[3], data[4], data[5]]);
                        if field_len > 0 {
                            // Read the value as string and parse to int
                            let value_bytes = &data[6..6 + field_len as usize];
                            let value_str = String::from_utf8_lossy(value_bytes);
                            backend_pid = Some(
                                value_str
                                    .parse()
                                    .expect("Failed to parse backend_pid as integer"),
                            );
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

    let pid = backend_pid
        .unwrap_or_else(|| panic!("No backend_pid received from session '{session_name}'"));

    world
        .named_backend_pids
        .insert((session_name.clone(), pid_name), pid);
}

#[then(regex = r#"^we verify backend_pid from session "([^"]+)" is different from "([^"]+)"$"#)]
pub async fn verify_backend_pid_different(
    world: &mut DoormanWorld,
    session_name: String,
    pid_name: String,
) {
    // Get messages from the stored session messages
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{session_name}'"));

    // Find DataRow and extract current backend_pid
    let mut current_pid: Option<i32> = None;
    for (msg_type, data) in messages {
        match msg_type {
            'D' => {
                // DataRow - parse the integer value
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]);
                    if field_count >= 1 {
                        // Read first field length (4 bytes)
                        let field_len = i32::from_be_bytes([data[2], data[3], data[4], data[5]]);
                        if field_len > 0 {
                            // Read the value as string and parse to int
                            let value_bytes = &data[6..6 + field_len as usize];
                            let value_str = String::from_utf8_lossy(value_bytes);
                            current_pid = Some(
                                value_str
                                    .parse()
                                    .expect("Failed to parse backend_pid as integer"),
                            );
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

    let current = current_pid
        .unwrap_or_else(|| panic!("No backend_pid received from session '{session_name}'"));

    let stored = world
        .named_backend_pids
        .get(&(session_name.clone(), pid_name.clone()))
        .unwrap_or_else(|| panic!("No stored backend_pid with name '{pid_name}'"));

    assert_ne!(
        current, *stored,
        "Backend PID should have changed but is still {current} (stored as '{pid_name}')"
    );
}

#[when(regex = r#"^we sleep for (\d+) milliseconds$"#)]
pub async fn sleep_for_milliseconds(_world: &mut DoormanWorld, ms: String) {
    let duration = ms.parse::<u64>().expect("Invalid sleep duration");
    tokio::time::sleep(tokio::time::Duration::from_millis(duration)).await;
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" without waiting$"#)]
pub async fn send_simple_query_to_session_without_waiting(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");
    // Don't wait for response - just send the query
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" expecting error$"#)]
pub async fn send_simple_query_to_session_expecting_error(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read all messages until ReadyForQuery or error and store them
    let messages = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");

    world
        .session_messages
        .insert(session_name.clone(), messages);
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" expecting error after ready$"#
)]
pub async fn send_simple_query_to_session_expecting_error_after_ready(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read all messages until ReadyForQuery AND any additional messages after (like ErrorResponse)
    let messages = conn
        .read_all_messages_until_ready_and_more()
        .await
        .expect("Failed to read messages");

    world
        .session_messages
        .insert(session_name.clone(), messages);
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" expecting connection close$"#
)]
pub async fn send_simple_query_to_session_expecting_connection_close(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    // Try to send query - may fail if connection already closed
    if conn.send_simple_query(&query).await.is_err() {
        // Connection already closed - this is expected
        return;
    }

    // Try to read response - should fail with connection reset or return error
    match conn.read_all_messages_until_ready().await {
        Ok(messages) => {
            // Check if we got an error response (pooler shutdown message)
            let has_error = messages.iter().any(|(msg_type, _)| *msg_type == 'E');
            if has_error {
                // Store messages for potential further inspection
                world
                    .session_messages
                    .insert(session_name.clone(), messages);
                return;
            }
            panic!(
                "Expected connection close or error for session '{session_name}', but got successful response"
            );
        }
        Err(_) => {
            // Connection closed - this is expected
        }
    }
}

#[then(regex = r#"^session "([^"]+)" should receive error containing "([^"]+)"$"#)]
pub async fn session_should_receive_error_containing(
    world: &mut DoormanWorld,
    session_name: String,
    expected_text: String,
) {
    // Get messages from the stored session messages
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{session_name}'"));

    // Find ErrorResponse in the messages
    let mut found_error: Option<String> = None;
    for (msg_type, data) in messages {
        if *msg_type == 'E' {
            // ErrorResponse - parse the error message
            let error_str = String::from_utf8_lossy(data).to_string();
            found_error = Some(error_str);
            break;
        }
    }

    let error_msg = found_error.unwrap_or_else(|| {
        panic!(
            "No ErrorResponse received from session '{session_name}', expected error containing '{expected_text}'"
        )
    });

    assert!(
        error_msg
            .to_lowercase()
            .contains(&expected_text.to_lowercase()),
        "Session '{session_name}': expected error containing '{expected_text}', got '{error_msg}'"
    );
}

#[then(
    regex = r#"^session "([^"]+)" should receive error containing "([^"]+)" with code "([^"]+)"$"#
)]
pub async fn session_should_receive_error_containing_with_code(
    world: &mut DoormanWorld,
    session_name: String,
    expected_text: String,
    expected_code: String,
) {
    // Get messages from the stored session messages
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{session_name}'"));

    // Find ErrorResponse in the messages
    let mut found_error: Option<(String, String)> = None;
    for (msg_type, data) in messages {
        if *msg_type == 'E' {
            // ErrorResponse - parse the error message and code
            // Format: S<severity>\0 V<severity>\0 C<code>\0 M<message>\0 ... \0
            let mut code = String::new();
            let mut message = String::new();
            let mut i = 0;
            while i < data.len() {
                let field_type = data[i] as char;
                if field_type == '\0' {
                    break;
                }
                i += 1;
                let start = i;
                while i < data.len() && data[i] != 0 {
                    i += 1;
                }
                let value = String::from_utf8_lossy(&data[start..i]).to_string();
                i += 1; // skip null terminator
                match field_type {
                    'C' => code = value,
                    'M' => message = value,
                    _ => {}
                }
            }
            found_error = Some((message, code));
            break;
        }
    }

    let (error_msg, error_code) = found_error.unwrap_or_else(|| {
        panic!(
            "No ErrorResponse received from session '{session_name}', expected error containing '{expected_text}' with code '{expected_code}'"
        )
    });

    assert!(
        error_msg
            .to_lowercase()
            .contains(&expected_text.to_lowercase()),
        "Session '{session_name}': expected error containing '{expected_text}', got '{error_msg}'"
    );

    assert_eq!(
        error_code, expected_code,
        "Session '{session_name}': expected error code '{expected_code}', got '{error_code}'"
    );
}

#[when(
    regex = r#"^we create session "([^"]+)" to postgres as "([^"]+)" with password "([^"]*)" and database "([^"]+)"$"#
)]
pub async fn create_named_session_to_postgres(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
    database: String,
) {
    let pg_port = world.pg_port.expect("PostgreSQL not started");
    let pg_addr = format!("127.0.0.1:{pg_port}");

    // Connect to PostgreSQL directly
    let mut conn = PgConnection::connect(&pg_addr)
        .await
        .expect("Failed to connect to PostgreSQL");
    conn.send_startup(&user, &database)
        .await
        .expect("Failed to send startup to PostgreSQL");
    conn.authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to PostgreSQL");

    world.named_sessions.insert(session_name, conn);
}

#[when(
    regex = r#"^we send CopyFromStdin "([^"]+)" with data "([^"]*)" to session "([^"]+)" expecting error$"#
)]
pub async fn send_copy_from_stdin_to_session_expecting_error(
    world: &mut DoormanWorld,
    query: String,
    data: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    // Unescape the data string (handle \t and \n)
    let unescaped_data = data.replace("\\t", "\t").replace("\\n", "\n");

    // Send the COPY command via simple query
    conn.send_simple_query(&query)
        .await
        .expect("Failed to send COPY query");

    // Read initial response (should be CopyInResponse 'G' or ErrorResponse 'E')
    let (msg_type, msg_data) = conn
        .read_message()
        .await
        .expect("Failed to read COPY response");

    let mut messages: Vec<(char, Vec<u8>)> = Vec::new();

    // If we got CopyInResponse ('G'), send the data and CopyDone
    if msg_type == 'G' {
        // Send copy data
        if !unescaped_data.is_empty() {
            conn.send_copy_data(unescaped_data.as_bytes())
                .await
                .expect("Failed to send CopyData");
        }
        conn.send_copy_done()
            .await
            .expect("Failed to send CopyDone");
    } else {
        // Error response - store it
        messages.push((msg_type, msg_data));
    }

    // Read remaining messages until ReadyForQuery
    let remaining = conn
        .read_all_messages_until_ready()
        .await
        .expect("Failed to read messages");
    messages.extend(remaining);

    world.session_messages.insert(session_name, messages);
}

// Admin console (pgbouncer database) steps

#[when(
    regex = r#"^we create admin session "([^"]+)" to pg_doorman as "([^"]+)" with password "([^"]*)"$"#
)]
pub async fn create_admin_session(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{doorman_port}");

    // Connect to pg_doorman admin console (database = pgbouncer)
    let mut conn = PgConnection::connect(&doorman_addr)
        .await
        .expect("Failed to connect to pg_doorman admin");
    conn.send_startup(&user, "pgbouncer")
        .await
        .expect("Failed to send startup to pg_doorman admin");
    conn.authenticate(&user, &password)
        .await
        .expect("Failed to authenticate to pg_doorman admin");

    world.named_sessions.insert(session_name, conn);
}

#[when(regex = r#"^we execute "([^"]+)" on admin session "([^"]+)" and store row count$"#)]
pub async fn execute_admin_query_and_store_row_count(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read messages and count DataRow messages
    let mut row_count = 0;
    loop {
        let (msg_type, _data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'T' => {
                // RowDescription - skip
            }
            'D' => {
                // DataRow - count it
                row_count += 1;
            }
            'C' => {
                // CommandComplete - skip
            }
            'Z' => {
                // ReadyForQuery - done
                break;
            }
            'E' => {
                panic!("Error received from admin session '{session_name}': {_data:?}");
            }
            _ => {
                // Other messages - skip
            }
        }
    }

    // Store row count in session_backend_pids (reusing existing field for simplicity)
    world
        .session_backend_pids
        .insert(format!("{session_name}_row_count"), row_count);
}

#[then(regex = r#"^admin session "([^"]+)" row count should be (\d+)$"#)]
pub async fn verify_admin_row_count(
    world: &mut DoormanWorld,
    session_name: String,
    expected_count: i32,
) {
    let key = format!("{session_name}_row_count");
    let actual_count = world
        .session_backend_pids
        .get(&key)
        .unwrap_or_else(|| panic!("No row count stored for session '{session_name}'"));

    assert_eq!(
        *actual_count, expected_count,
        "Admin session '{session_name}': expected {expected_count} rows, got {actual_count}"
    );
}

#[when(regex = r#"^we abort TCP connection for session "([^"]+)"$"#)]
pub async fn abort_session_tcp_connection(world: &mut DoormanWorld, session_name: String) {
    let conn = world
        .named_sessions
        .remove(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    // Abruptly close the TCP connection
    conn.abort_connection().await;
}

#[then(regex = r#"^admin session "([^"]+)" row count should be greater than (\d+)$"#)]
pub async fn verify_admin_row_count_greater_than(
    world: &mut DoormanWorld,
    session_name: String,
    min_count: i32,
) {
    let key = format!("{session_name}_row_count");
    let actual_count = world
        .session_backend_pids
        .get(&key)
        .unwrap_or_else(|| panic!("No row count stored for session '{session_name}'"));

    assert!(
        *actual_count > min_count,
        "Admin session '{session_name}': expected more than {min_count} rows, got {actual_count}"
    );
}

#[then(regex = r#"^admin session "([^"]+)" row count should be greater than or equal to (\d+)$"#)]
pub async fn verify_admin_row_count_greater_or_equal(
    world: &mut DoormanWorld,
    session_name: String,
    min_count: i32,
) {
    let key = format!("{session_name}_row_count");
    let actual_count = world
        .session_backend_pids
        .get(&key)
        .unwrap_or_else(|| panic!("No row count stored for session '{session_name}'"));

    assert!(
        *actual_count >= min_count,
        "Admin session '{session_name}': expected at least {min_count} rows, got {actual_count}"
    );
}

#[when(regex = r#"^we execute "([^"]+)" on admin session "([^"]+)" expecting possible error$"#)]
pub async fn execute_admin_query_expecting_possible_error(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read messages and count DataRow messages, but don't panic on error
    let mut row_count = 0;
    let mut got_error = false;
    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'T' => {
                // RowDescription - skip
            }
            'D' => {
                // DataRow - count it
                row_count += 1;
            }
            'C' => {
                // CommandComplete - skip
            }
            'Z' => {
                // ReadyForQuery - done
                break;
            }
            'E' => {
                // Error - log it but don't panic
                got_error = true;
                eprintln!(
                    "Admin session '{}' received error (expected): {:?}",
                    session_name,
                    String::from_utf8_lossy(&data)
                );
            }
            _ => {
                // Other messages - skip
            }
        }
    }

    // Store row count (will be 0 if error)
    world
        .session_backend_pids
        .insert(format!("{session_name}_row_count"), row_count);

    // Store error flag
    world.session_backend_pids.insert(
        format!("{session_name}_got_error"),
        if got_error { 1 } else { 0 },
    );
}

#[when(regex = r#"^we execute "([^"]+)" on admin session "([^"]+)" and store response$"#)]
pub async fn execute_admin_query_and_store_response(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read messages and collect all response content as structured table
    let mut response_content = String::new();
    let mut headers: Vec<String> = Vec::new();
    let mut is_first_row = true;

    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'T' => {
                // RowDescription - parse column names
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]) as usize;
                    let mut pos = 2;
                    for _ in 0..field_count {
                        // Read column name (null-terminated string)
                        if let Some(null_pos) = data[pos..].iter().position(|&b| b == 0) {
                            let col_name = String::from_utf8_lossy(&data[pos..pos + null_pos]);
                            headers.push(col_name.to_string());
                            pos += null_pos + 1;
                            // Skip: table OID (4), column attr (2), type OID (4), type size (2), type mod (4), format (2) = 18 bytes
                            pos += 18;
                        }
                    }
                    // Write header line
                    response_content.push_str(&headers.join("|"));
                    response_content.push('\n');
                }
            }
            'D' => {
                // DataRow - extract text content
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]) as usize;
                    let mut pos = 2;
                    let mut row_values: Vec<String> = Vec::new();
                    for _ in 0..field_count {
                        if pos + 4 <= data.len() {
                            let field_len = i32::from_be_bytes([
                                data[pos],
                                data[pos + 1],
                                data[pos + 2],
                                data[pos + 3],
                            ]);
                            pos += 4;
                            if field_len > 0 && pos + field_len as usize <= data.len() {
                                let value =
                                    String::from_utf8_lossy(&data[pos..pos + field_len as usize]);
                                row_values.push(value.to_string());
                                pos += field_len as usize;
                            } else if field_len == -1 {
                                // NULL value
                                row_values.push(String::new());
                            } else {
                                row_values.push(String::new());
                            }
                        }
                    }
                    if !is_first_row {
                        response_content.push('\n');
                    }
                    response_content.push_str(&row_values.join("|"));
                    is_first_row = false;
                }
            }
            'C' => {
                // CommandComplete - extract tag
                if let Some(null_pos) = data.iter().position(|&b| b == 0) {
                    let tag = String::from_utf8_lossy(&data[..null_pos]);
                    response_content.push('\n');
                    response_content.push_str(&tag);
                }
            }
            'A' => {
                // NotificationResponse (Async notification) - this is what show help returns
                // Format: process_id (4 bytes) + channel (null-terminated) + payload (null-terminated)
                if data.len() >= 4 {
                    let mut pos = 4; // skip process_id
                                     // Read channel name
                    if let Some(null_pos) = data[pos..].iter().position(|&b| b == 0) {
                        let channel = String::from_utf8_lossy(&data[pos..pos + null_pos]);
                        response_content.push_str(&channel);
                        response_content.push(' ');
                        pos += null_pos + 1;
                        // Read payload
                        if let Some(null_pos2) = data[pos..].iter().position(|&b| b == 0) {
                            let payload = String::from_utf8_lossy(&data[pos..pos + null_pos2]);
                            response_content.push_str(&payload);
                            response_content.push(' ');
                        }
                    }
                }
            }
            'Z' => {
                // ReadyForQuery - done
                break;
            }
            'E' => {
                // Error - store error message
                let error_str = String::from_utf8_lossy(&data);
                response_content.push_str("ERROR: ");
                response_content.push_str(&error_str);
            }
            _ => {
                // Other messages - skip
            }
        }
    }

    // Store response content
    world
        .session_messages
        .insert(session_name, vec![('R', response_content.into_bytes())]);
}

#[then(regex = r#"^admin session "([^"]+)" response should contain "([^"]+)"$"#)]
pub async fn verify_admin_response_contains(
    world: &mut DoormanWorld,
    session_name: String,
    expected_text: String,
) {
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No response stored for session '{session_name}'"));

    let response_content = if let Some((_, data)) = messages.first() {
        String::from_utf8_lossy(data).to_string()
    } else {
        String::new()
    };

    assert!(
        response_content
            .to_uppercase()
            .contains(&expected_text.to_uppercase()),
        "Admin session '{session_name}': expected response to contain '{expected_text}', got '{response_content}'"
    );
}

#[then(regex = r#"^admin session "([^"]+)" column "([^"]+)" should be between (\d+) and (\d+)$"#)]
pub async fn verify_admin_column_in_range(
    world: &mut DoormanWorld,
    session_name: String,
    column_name: String,
    min_value: u64,
    max_value: u64,
) {
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No response stored for session '{session_name}'"));

    let response_content = if let Some((_, data)) = messages.first() {
        String::from_utf8_lossy(data).to_string()
    } else {
        panic!("No response content for session '{session_name}'");
    };

    // Parse the response as a table (header row + data rows)
    // Format can be either "col1|col2|col3\nval1|val2|val3\n..." or space-separated
    let lines: Vec<&str> = response_content.lines().collect();
    if lines.is_empty() {
        panic!(
            "Admin session '{session_name}': empty response, cannot find column '{column_name}'"
        );
    }

    // Determine separator: if first line contains '|', use it; otherwise use whitespace
    let use_pipe = lines[0].contains('|');

    // Find column index in header
    let headers: Vec<&str> = if use_pipe {
        lines[0].split('|').map(|s| s.trim()).collect()
    } else {
        lines[0].split_whitespace().collect()
    };

    let col_idx = headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case(&column_name))
        .unwrap_or_else(|| {
            panic!(
                "Admin session '{session_name}': column '{column_name}' not found in headers: {headers:?}"
            )
        });

    // Get value from first data row
    if lines.len() < 2 {
        panic!("Admin session '{session_name}': no data rows in response");
    }

    let values: Vec<&str> = if use_pipe {
        lines[1].split('|').map(|s| s.trim()).collect()
    } else {
        lines[1].split_whitespace().collect()
    };

    if col_idx >= values.len() {
        panic!(
            "Admin session '{session_name}': column index {col_idx} out of bounds for row: {values:?}"
        );
    }

    let value_str = values[col_idx];
    let value: u64 = value_str.parse().unwrap_or_else(|_| {
        panic!(
            "Admin session '{session_name}': cannot parse '{value_str}' as u64 for column '{column_name}'"
        )
    });

    assert!(
        value >= min_value && value <= max_value,
        "Admin session '{session_name}': column '{column_name}' value {value} is not between {min_value} and {max_value}"
    );

    eprintln!(
        "Admin session '{session_name}': column '{column_name}' = {value} (expected between {min_value} and {max_value})"
    );
}

// Cancel request steps

#[when(
    regex = r#"^we create session "([^"]+)" to pg_doorman as "([^"]+)" with password "([^"]*)" and database "([^"]+)" and store backend key$"#
)]
pub async fn create_named_session_with_backend_key(
    world: &mut DoormanWorld,
    session_name: String,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{doorman_port}");

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

    // Store backend key data (process_id and secret_key from BackendKeyData)
    if let (Some(process_id), Some(secret_key)) = (conn.get_process_id(), conn.get_secret_key()) {
        world
            .session_backend_pids
            .insert(session_name.clone(), process_id);
        world
            .session_secret_keys
            .insert(session_name.clone(), secret_key);
        eprintln!(
            "Session '{session_name}': stored backend_pid={process_id}, secret_key={secret_key}"
        );
    } else {
        panic!("Session '{session_name}': BackendKeyData not received during authentication");
    }

    world.named_sessions.insert(session_name, conn);
}

#[when(
    regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" without waiting for response$"#
)]
pub async fn send_simple_query_to_session_no_wait(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Don't wait for response - the query is running
    eprintln!("Session '{session_name}': sent query '{query}' without waiting");
}

#[when(regex = r#"^we send cancel request for session "([^"]+)"$"#)]
pub async fn send_cancel_request_for_session(world: &mut DoormanWorld, session_name: String) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{doorman_port}");

    let process_id = world
        .session_backend_pids
        .get(&session_name)
        .unwrap_or_else(|| panic!("No backend_pid stored for session '{session_name}'"));

    let secret_key = world
        .session_secret_keys
        .get(&session_name)
        .unwrap_or_else(|| panic!("No secret_key stored for session '{session_name}'"));

    eprintln!(
        "Sending cancel request for session '{session_name}': process_id={process_id}, secret_key={secret_key}"
    );

    PgConnection::send_cancel_request(&doorman_addr, *process_id, *secret_key)
        .await
        .expect("Failed to send cancel request");

    // Give the server a moment to process the cancel
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
}

#[then(regex = r#"^session "([^"]+)" should receive cancel error containing "([^"]+)"$"#)]
pub async fn session_should_receive_cancel_error(
    world: &mut DoormanWorld,
    session_name: String,
    expected_text: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    // Read messages until we get an error or ReadyForQuery
    let mut error_found = false;
    let mut error_message = String::new();

    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'E' => {
                // Error message - parse it
                error_message = String::from_utf8_lossy(&data).to_string();
                error_found = true;
                eprintln!("Session '{session_name}': received error: {error_message}");
            }
            'Z' => {
                // ReadyForQuery - done
                break;
            }
            _ => {
                // Other messages - continue
            }
        }
    }

    assert!(
        error_found,
        "Session '{session_name}': expected to receive an error, but none was received"
    );

    assert!(
        error_message
            .to_lowercase()
            .contains(&expected_text.to_lowercase()),
        "Session '{session_name}': expected error to contain '{expected_text}', got '{error_message}'"
    );
}

#[then(regex = r#"^session "([^"]+)" should complete without error$"#)]
pub async fn session_should_complete_without_error(world: &mut DoormanWorld, session_name: String) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    // Read messages until ReadyForQuery, checking for errors
    let mut error_found = false;
    let mut error_message = String::new();

    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'E' => {
                // Error message - this is unexpected
                error_message = String::from_utf8_lossy(&data).to_string();
                error_found = true;
                eprintln!("Session '{session_name}': received unexpected error: {error_message}");
            }
            'Z' => {
                // ReadyForQuery - done
                eprintln!("Session '{session_name}': query completed successfully");
                break;
            }
            _ => {
                // Other messages - continue (T=RowDescription, D=DataRow, C=CommandComplete, etc.)
            }
        }
    }

    assert!(
        !error_found,
        "Session '{session_name}': expected query to complete without error, but got: {error_message}"
    );
}

// Buffer cleanup test steps

#[when(regex = r#"^we read (\d+) bytes from session "([^"]+)"$"#)]
pub async fn read_bytes_from_session(world: &mut DoormanWorld, bytes: usize, session_name: String) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    let bytes_read = conn
        .read_limited_bytes(bytes)
        .await
        .expect("Failed to read bytes from session");

    eprintln!("Session '{session_name}': read {bytes_read} bytes (requested {bytes})");
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and verify no stale data$"#)]
pub async fn send_query_and_verify_no_stale_data(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Read all messages and store them for verification
    let mut messages: Vec<(char, Vec<u8>)> = Vec::new();
    let mut data_content = String::new();

    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'T' => {
                // RowDescription - expected
                messages.push((msg_type, data));
            }
            'D' => {
                // DataRow - extract content
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]) as usize;
                    let mut pos = 2;
                    for _ in 0..field_count {
                        if pos + 4 <= data.len() {
                            let field_len = i32::from_be_bytes([
                                data[pos],
                                data[pos + 1],
                                data[pos + 2],
                                data[pos + 3],
                            ]);
                            pos += 4;
                            if field_len > 0 && pos + field_len as usize <= data.len() {
                                let value =
                                    String::from_utf8_lossy(&data[pos..pos + field_len as usize]);
                                data_content.push_str(&value);
                                pos += field_len as usize;
                            }
                        }
                    }
                }
                messages.push((msg_type, data));
            }
            'C' => {
                // CommandComplete - expected
                messages.push((msg_type, data));
            }
            'Z' => {
                // ReadyForQuery - done
                messages.push((msg_type, data));
                break;
            }
            'E' => {
                // Error - unexpected
                let error_str = String::from_utf8_lossy(&data);
                panic!("Session '{session_name}': received unexpected error: {error_str}");
            }
            _ => {
                // Other messages - store them
                messages.push((msg_type, data));
            }
        }
    }

    // Store the data content for later verification
    world
        .session_messages
        .insert(session_name.clone(), vec![('D', data_content.into_bytes())]);

    eprintln!(
        "Session '{}': received {} messages",
        session_name,
        messages.len()
    );
}

#[then(regex = r#"^session "([^"]+)" should have received clean response with marker "([^"]+)"$"#)]
pub async fn verify_clean_response_with_marker(
    world: &mut DoormanWorld,
    session_name: String,
    expected_marker: String,
) {
    let messages = world
        .session_messages
        .get(&session_name)
        .unwrap_or_else(|| panic!("No messages stored for session '{session_name}'"));

    let data_content = if let Some((_, data)) = messages.first() {
        String::from_utf8_lossy(data).to_string()
    } else {
        String::new()
    };

    // Verify that the response contains ONLY the expected marker
    // and no stale data (like 'X', 'A', 'B', 'C', 'T' repeated patterns from previous queries)
    assert!(
        data_content.contains(&expected_marker),
        "Session '{session_name}': expected response to contain marker '{expected_marker}', got '{data_content}'"
    );

    // Check for stale data patterns (large repeated characters from previous queries)
    let stale_patterns = ["XXXX", "AAAA", "BBBB", "CCCC", "TTTT"];
    for pattern in stale_patterns {
        assert!(
            !data_content.contains(pattern),
            "Session '{}': found stale data pattern '{}' in response '{}' - buffer was not cleaned!",
            session_name,
            pattern,
            &data_content[..std::cmp::min(200, data_content.len())]
        );
    }

    eprintln!("Session '{session_name}': verified clean response with marker '{expected_marker}'");
}

// =============================================================================
// Flush timeout protocol violation test steps
// =============================================================================

/// Send Sync to a session without waiting for the server response.
/// Used when we expect the server roundtrip to timeout (e.g., PostgreSQL is frozen).
#[when(regex = r#"^we send Sync to session "([^"]+)" without waiting for response$"#)]
pub async fn send_sync_to_session_no_wait(world: &mut DoormanWorld, session_name: String) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    conn.send_sync().await.expect("Failed to send Sync");
    eprintln!("Session '{session_name}': Sync sent (not waiting for response)");
}

/// Freeze PostgreSQL with SIGSTOP to block all I/O.
/// Sends SIGSTOP to the entire process group (postmaster + all backends).
/// pg_doorman's TCP writes will eventually block when kernel buffers fill.
#[when(regex = r#"^we freeze PostgreSQL with SIGSTOP$"#)]
pub async fn freeze_postgres(world: &mut DoormanWorld) {
    let db_path = world
        .pg_db_path
        .as_ref()
        .expect("PostgreSQL not started (no db_path)");
    let pid_file = db_path.join("postmaster.pid");
    let pid_content = std::fs::read_to_string(&pid_file)
        .unwrap_or_else(|e| panic!("Failed to read postmaster.pid at {pid_file:?}: {e}"));
    let pid: i32 = pid_content
        .lines()
        .next()
        .expect("Empty postmaster.pid")
        .trim()
        .parse()
        .expect("Invalid PID in postmaster.pid");

    // Send SIGSTOP to ALL PostgreSQL processes: the postmaster and all backend
    // children. We use pgrep to find child processes because process group
    // signaling may not cover backends on all platforms.
    eprintln!("Freezing PostgreSQL: finding all child processes of postmaster PID {pid}");

    // Find all child processes using pgrep (works on both Linux and macOS)
    let children = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output();

    let mut stopped_pids = vec![pid];
    if let Ok(output) = children {
        let pids_str = String::from_utf8_lossy(&output.stdout);
        for line in pids_str.lines() {
            if let Ok(child_pid) = line.trim().parse::<i32>() {
                stopped_pids.push(child_pid);
            }
        }
    }

    // SIGSTOP all processes (children first, then postmaster)
    for &p in stopped_pids.iter().rev() {
        eprintln!("Sending SIGSTOP to PG process {p}");
        unsafe {
            libc::kill(p, libc::SIGSTOP);
        }
    }

    eprintln!(
        "Frozen {} PostgreSQL processes: {:?}",
        stopped_pids.len(),
        stopped_pids
    );
    // Give the OS a moment to actually stop all processes
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// Unfreeze PostgreSQL with SIGCONT after a SIGSTOP.
/// Sends SIGCONT to postmaster and all its child processes.
#[when(regex = r#"^we unfreeze PostgreSQL with SIGCONT$"#)]
pub async fn unfreeze_postgres(world: &mut DoormanWorld) {
    let db_path = world
        .pg_db_path
        .as_ref()
        .expect("PostgreSQL not started (no db_path)");
    let pid_file = db_path.join("postmaster.pid");
    let pid_content = std::fs::read_to_string(&pid_file)
        .unwrap_or_else(|e| panic!("Failed to read postmaster.pid at {pid_file:?}: {e}"));
    let pid: i32 = pid_content
        .lines()
        .next()
        .expect("Empty postmaster.pid")
        .trim()
        .parse()
        .expect("Invalid PID in postmaster.pid");

    // Find all child processes
    let children = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output();

    let mut pids = vec![pid];
    if let Ok(output) = children {
        let pids_str = String::from_utf8_lossy(&output.stdout);
        for line in pids_str.lines() {
            if let Ok(child_pid) = line.trim().parse::<i32>() {
                pids.push(child_pid);
            }
        }
    }

    // SIGCONT all processes (postmaster first, then children)
    for &p in &pids {
        eprintln!("Sending SIGCONT to PG process {p}");
        unsafe {
            libc::kill(p, libc::SIGCONT);
        }
    }

    eprintln!("Unfrozen {} PostgreSQL processes", pids.len());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// Send a large batch of Parse messages to a session to fill TCP send buffers.
/// Each Parse contains a query padded to ~query_size bytes.
#[when(
    regex = r#"^we send large batch of (\d+) Parse messages with (\d+)KB queries to session "([^"]+)"$"#
)]
pub async fn send_large_parse_batch(
    world: &mut DoormanWorld,
    count: String,
    query_kb: String,
    session_name: String,
) {
    let count: usize = count.parse().expect("Invalid count");
    let query_kb: usize = query_kb.parse().expect("Invalid query_kb");
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    // Build a large SQL query padded with comments
    let base_query = "SELECT 1";
    let padding_size = query_kb * 1024 - base_query.len() - 6; // 6 for "/* */"
    let padding: String = "x".repeat(padding_size);
    let large_query = format!("{base_query}/* {padding} */");

    eprintln!(
        "Sending {} Parse messages (~{}KB each, ~{}MB total) to session '{}'",
        count,
        query_kb,
        (count * query_kb) / 1024,
        session_name
    );

    for i in 0..count {
        let stmt_name = format!("flush_test_{i}");
        if let Err(e) = conn.send_parse(&stmt_name, &large_query).await {
            eprintln!("Write failed at message {i} (expected after buffer fills): {e}");
            break;
        }
    }

    eprintln!("Finished sending batch to session '{session_name}'");
}

/// Send a large SimpleQuery to a session to fill TCP send buffers.
/// The query is padded with a SQL comment to reach the desired size.
/// Does not wait for a response (used when we expect a flush timeout).
#[when(
    regex = r#"^we send large SimpleQuery with (\d+)KB padding to session "([^"]+)" without waiting$"#
)]
pub async fn send_large_simple_query_no_wait(
    world: &mut DoormanWorld,
    size_kb: String,
    session_name: String,
) {
    let size_kb: usize = size_kb.parse().expect("Invalid size_kb");
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    let base_query = "SELECT 1";
    let padding_size = size_kb * 1024 - base_query.len() - 6; // 6 for "/* */"
    let padding: String = "x".repeat(padding_size);
    let large_query = format!("{base_query}/* {padding} */");

    eprintln!("Sending large SimpleQuery (~{size_kb}KB) to session '{session_name}'");

    if let Err(e) = conn.send_simple_query(&large_query).await {
        eprintln!("Write failed for large SimpleQuery (expected after buffer fills): {e}");
    }
}

/// Verify that the client received a proper ErrorResponse ('E') from pg_doorman
/// when flush timeout occurred, rather than a bare TCP close / unexpected EOF.
///
/// The test PASSES if the client receives an ErrorResponse message.
/// The test FAILS if the client gets only EOF/connection reset (protocol violation).
#[then(
    regex = r#"^session "([^"]+)" should receive ErrorResponse or connection close with error$"#
)]
pub async fn verify_error_response_on_flush_timeout(
    world: &mut DoormanWorld,
    session_name: String,
) {
    let conn = world
        .named_sessions
        .get_mut(&session_name)
        .unwrap_or_else(|| panic!("Session '{session_name}' not found"));

    // Try to read any message with a timeout.
    // We expect either:
    // - ErrorResponse ('E') - correct behavior (what we want)
    // - Connection reset / EOF - the bug (protocol violation for drivers like Npgsql)
    let mut got_error_response = false;
    let mut got_eof = false;
    let mut messages_received = Vec::new();

    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), conn.read_message()).await {
            Ok(Ok((msg_type, data))) => {
                let desc = format!("type='{}' len={}", msg_type, data.len());
                eprintln!("Session '{session_name}': received message {desc}");
                messages_received.push((msg_type, data));

                if msg_type == 'E' {
                    got_error_response = true;
                }
                if msg_type == 'Z' {
                    // ReadyForQuery - done reading
                    break;
                }
            }
            Ok(Err(e)) => {
                // Connection error (EOF, reset, etc.)
                eprintln!(
                    "Session '{session_name}': connection error (this is the bug - no ErrorResponse sent): {e}"
                );
                got_eof = true;
                break;
            }
            Err(_) => {
                // Timeout - no more messages
                eprintln!("Session '{session_name}': read timeout (no more messages)");
                break;
            }
        }
    }

    // Store results for debugging
    world
        .session_messages
        .insert(session_name.clone(), messages_received);

    // The assertion: we MUST have received an ErrorResponse.
    // If we only got EOF/reset, that's the protocol violation bug.
    assert!(
        got_error_response,
        "Session '{}': expected ErrorResponse ('E') from pg_doorman on flush timeout, \
         but got {}. This is the protocol violation bug - client receives bare TCP close \
         without a proper PostgreSQL error message.",
        session_name,
        if got_eof {
            "connection EOF/reset (no PostgreSQL message at all)"
        } else {
            "no ErrorResponse in received messages"
        }
    );

    eprintln!("Session '{session_name}': correctly received ErrorResponse on flush timeout");
}
