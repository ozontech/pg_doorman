use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{then, when};

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
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

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

#[when(regex = r#"^we execute "([^"]+)" on admin session "([^"]+)"$"#)]
pub async fn execute_admin_command(world: &mut DoormanWorld, query: String, session_name: String) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'Z' => break,
            'E' => {
                let err = String::from_utf8_lossy(&data);
                panic!(
                    "Error from admin session '{}' on '{}': {}",
                    session_name, query, err
                );
            }
            _ => {}
        }
    }
}

#[when(regex = r#"^we execute "([^"]+)" on admin session "([^"]+)" and store row count$"#)]
pub async fn execute_admin_query_and_store_row_count(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

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
                panic!(
                    "Error received from admin session '{}': {:?}",
                    session_name, _data
                );
            }
            _ => {
                // Other messages - skip
            }
        }
    }

    // Store row count in session_backend_pids (reusing existing field for simplicity)
    world
        .session_backend_pids
        .insert(format!("{}_row_count", session_name), row_count);
}

#[when(regex = r#"^we execute "([^"]+)" on admin session "([^"]+)" expecting possible error$"#)]
pub async fn execute_admin_query_expecting_possible_error(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

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
        .insert(format!("{}_row_count", session_name), row_count);

    // Store error flag
    world.session_backend_pids.insert(
        format!("{}_got_error", session_name),
        if got_error { 1 } else { 0 },
    );
}

#[when(regex = r#"^we execute "([^"]+)" on admin session "([^"]+)" and store response$"#)]
pub async fn execute_admin_query_and_store_response(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    let mut response_content = String::new();
    let mut headers: Vec<String> = Vec::new();
    let mut is_first_row = true;

    loop {
        let (msg_type, data) = conn.read_message().await.expect("Failed to read message");

        match msg_type {
            'T' => {
                if data.len() >= 2 {
                    let field_count = i16::from_be_bytes([data[0], data[1]]) as usize;
                    let mut pos = 2;
                    for _ in 0..field_count {
                        if let Some(null_pos) = data[pos..].iter().position(|&b| b == 0) {
                            let col_name = String::from_utf8_lossy(&data[pos..pos + null_pos]);
                            headers.push(col_name.to_string());
                            pos += null_pos + 1;
                            pos += 18; // table OID(4) + col attr(2) + type OID(4) + type size(2) + type mod(4) + format(2)
                        }
                    }
                    response_content.push_str(&headers.join("|"));
                    response_content.push('\n');
                }
            }
            'D' => {
                let row_values = super::helpers::parse_datarow_fields(&data);
                if !is_first_row {
                    response_content.push('\n');
                }
                response_content.push_str(&row_values.join("|"));
                is_first_row = false;
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

#[then(regex = r#"^admin session "([^"]+)" row count should be (\d+)$"#)]
pub async fn verify_admin_row_count(
    world: &mut DoormanWorld,
    session_name: String,
    expected_count: i32,
) {
    let key = format!("{}_row_count", session_name);
    let actual_count = world
        .session_backend_pids
        .get(&key)
        .unwrap_or_else(|| panic!("No row count stored for session '{}'", session_name));

    assert_eq!(
        *actual_count, expected_count,
        "Admin session '{}': expected {} rows, got {}",
        session_name, expected_count, actual_count
    );
}

#[then(regex = r#"^admin session "([^"]+)" row count should be greater than (\d+)$"#)]
pub async fn verify_admin_row_count_greater_than(
    world: &mut DoormanWorld,
    session_name: String,
    min_count: i32,
) {
    let key = format!("{}_row_count", session_name);
    let actual_count = world
        .session_backend_pids
        .get(&key)
        .unwrap_or_else(|| panic!("No row count stored for session '{}'", session_name));

    assert!(
        *actual_count > min_count,
        "Admin session '{}': expected more than {} rows, got {}",
        session_name,
        min_count,
        actual_count
    );
}

#[then(regex = r#"^admin session "([^"]+)" row count should be greater than or equal to (\d+)$"#)]
pub async fn verify_admin_row_count_greater_or_equal(
    world: &mut DoormanWorld,
    session_name: String,
    min_count: i32,
) {
    let key = format!("{}_row_count", session_name);
    let actual_count = world
        .session_backend_pids
        .get(&key)
        .unwrap_or_else(|| panic!("No row count stored for session '{}'", session_name));

    assert!(
        *actual_count >= min_count,
        "Admin session '{}': expected at least {} rows, got {}",
        session_name,
        min_count,
        actual_count
    );
}

#[then(regex = r#"^admin session "([^"]+)" response should contain "([^"]+)"$"#)]
pub async fn verify_admin_response_contains(
    world: &mut DoormanWorld,
    session_name: String,
    expected_text: String,
) {
    let response = super::helpers::get_admin_response(&world.session_messages, &session_name);

    assert!(
        response
            .to_uppercase()
            .contains(&expected_text.to_uppercase()),
        "Admin session '{}': expected response to contain '{}', got '{}'",
        session_name,
        expected_text,
        response
    );
}

#[then(regex = r#"^admin session "([^"]+)" response should not contain "([^"]+)"$"#)]
pub async fn verify_admin_response_not_contains(
    world: &mut DoormanWorld,
    session_name: String,
    unexpected_text: String,
) {
    let response = super::helpers::get_admin_response(&world.session_messages, &session_name);

    assert!(
        !response
            .to_uppercase()
            .contains(&unexpected_text.to_uppercase()),
        "Admin session '{}': expected response NOT to contain '{}', but got '{}'",
        session_name,
        unexpected_text,
        response
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
    let response = super::helpers::get_admin_response(&world.session_messages, &session_name);
    let lines: Vec<&str> = response.lines().collect();
    assert!(
        lines.len() >= 2,
        "Admin session '{}': need header + data row",
        session_name
    );

    let (col_idx, use_pipe) = super::helpers::find_column_index(lines[0], &column_name);
    let values = super::helpers::split_row(lines[1], use_pipe);

    let value: u64 = values[col_idx].parse().unwrap_or_else(|_| {
        panic!(
            "Admin session '{}': cannot parse '{}' as u64 for column '{}'",
            session_name, values[col_idx], column_name
        )
    });

    assert!(
        value >= min_value && value <= max_value,
        "Admin session '{}': column '{}' value {} is not between {} and {}",
        session_name,
        column_name,
        value,
        min_value,
        max_value
    );
}

#[then(
    regex = r#"^admin session "([^"]+)" column "([^"]+)" for row with "([^"]+)" = "([^"]+)" should be between (\d+) and (\d+)$"#
)]
pub async fn verify_admin_column_in_range_for_row(
    world: &mut DoormanWorld,
    session_name: String,
    column_name: String,
    filter_column: String,
    filter_value: String,
    min_value: u64,
    max_value: u64,
) {
    let response = super::helpers::get_admin_response(&world.session_messages, &session_name);
    let lines: Vec<&str> = response.lines().collect();
    assert!(
        !lines.is_empty(),
        "Admin session '{}': empty response",
        session_name
    );

    let (col_idx, use_pipe) = super::helpers::find_column_index(lines[0], &column_name);
    let (filter_col_idx, _) = super::helpers::find_column_index(lines[0], &filter_column);

    for line in &lines[1..] {
        let values = super::helpers::split_row(line, use_pipe);
        if filter_col_idx >= values.len() || col_idx >= values.len() {
            continue;
        }
        if values[filter_col_idx] != filter_value {
            continue;
        }

        let value: u64 = values[col_idx].parse().unwrap_or_else(|_| {
            panic!(
                "Admin session '{}': cannot parse '{}' as u64 for column '{}'",
                session_name, values[col_idx], column_name
            )
        });

        assert!(
            value >= min_value && value <= max_value,
            "Admin session '{}': column '{}' value {} (row {}={}) is not between {} and {}",
            session_name,
            column_name,
            value,
            filter_column,
            filter_value,
            min_value,
            max_value
        );
        return;
    }

    panic!(
        "Admin session '{}': no row found with {}='{}'",
        session_name, filter_column, filter_value
    );
}
