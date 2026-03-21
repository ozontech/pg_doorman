use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{then, when};

use super::helpers;

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

    let pg_addr = format!("127.0.0.1:{}", pg_port);
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

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

    let params = super::helpers::parse_bind_params(&params_str);

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

    let params = super::helpers::parse_bind_params(&params_str);

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

    let params = super::helpers::parse_bind_params(&params_str);

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

    let params = super::helpers::parse_bind_params(&params_str);

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
                helpers::format_message_details(*msg_type, data)
            ));
        }

        error_msg.push_str("\n=== pg_doorman messages ===\n");
        for (i, (msg_type, data)) in doorman_messages.iter().enumerate() {
            error_msg.push_str(&format!(
                "  [{}] {}\n",
                i,
                helpers::format_message_details(*msg_type, data)
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
            eprintln!("\n=== MESSAGE TYPE MISMATCH at position {} ===", i);
            eprintln!(
                "PostgreSQL: {}",
                helpers::format_message_details(*pg_type, pg_data)
            );
            eprintln!(
                "pg_doorman: {}",
                helpers::format_message_details(*doorman_type, doorman_data)
            );
            panic!(
                "Message {} type differs: PostgreSQL='{}', pg_doorman='{}'",
                i, pg_type, doorman_type
            );
        }

        // Check message length
        if pg_data.len() != doorman_data.len() {
            eprintln!("\n=== MESSAGE LENGTH MISMATCH at position {} ===", i);
            eprintln!(
                "PostgreSQL: {}",
                helpers::format_message_details(*pg_type, pg_data)
            );
            eprintln!(
                "pg_doorman: {}",
                helpers::format_message_details(*doorman_type, doorman_data)
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
        // For RowDescription ('T'), normalize table OIDs before comparison
        // because temp tables have different OIDs on different connections
        let (pg_data_normalized, doorman_data_normalized) = if *pg_type == 'T' {
            (
                helpers::normalize_row_description(pg_data),
                helpers::normalize_row_description(doorman_data),
            )
        } else {
            (pg_data.clone(), doorman_data.clone())
        };

        if pg_data_normalized != doorman_data_normalized {
            eprintln!("\n=== MESSAGE DATA MISMATCH at position {} ===", i);
            eprintln!(
                "PostgreSQL: {}",
                helpers::format_message_details(*pg_type, pg_data)
            );
            eprintln!(
                "pg_doorman: {}",
                helpers::format_message_details(*doorman_type, doorman_data)
            );

            // Find first difference
            for (pos, (pg_byte, doorman_byte)) in pg_data_normalized
                .iter()
                .zip(doorman_data_normalized.iter())
                .enumerate()
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
            helpers::format_message_details(*pg_type, pg_data)
        );
    }

    // Clear accumulated messages for next scenario
    world.pg_accumulated_messages.clear();
    world.doorman_accumulated_messages.clear();
}
