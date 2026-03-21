use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{then, when};

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

    // Store backend key data (process_id and secret_key from BackendKeyData)
    if let (Some(process_id), Some(secret_key)) = (conn.get_process_id(), conn.get_secret_key()) {
        world
            .session_backend_pids
            .insert(session_name.clone(), process_id);
        world
            .session_secret_keys
            .insert(session_name.clone(), secret_key);
        eprintln!(
            "Session '{}': stored backend_pid={}, secret_key={}",
            session_name, process_id, secret_key
        );
    } else {
        panic!(
            "Session '{}': BackendKeyData not received during authentication",
            session_name
        );
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
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_simple_query(&query)
        .await
        .expect("Failed to send query");

    // Don't wait for response - the query is running
    eprintln!(
        "Session '{}': sent query '{}' without waiting",
        session_name, query
    );
}

#[when(regex = r#"^we send cancel request for session "([^"]+)"$"#)]
pub async fn send_cancel_request_for_session(world: &mut DoormanWorld, session_name: String) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let doorman_addr = format!("127.0.0.1:{}", doorman_port);

    let process_id = world
        .session_backend_pids
        .get(&session_name)
        .unwrap_or_else(|| panic!("No backend_pid stored for session '{}'", session_name));

    let secret_key = world
        .session_secret_keys
        .get(&session_name)
        .unwrap_or_else(|| panic!("No secret_key stored for session '{}'", session_name));

    eprintln!(
        "Sending cancel request for session '{}': process_id={}, secret_key={}",
        session_name, process_id, secret_key
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
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

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
                eprintln!(
                    "Session '{}': received error: {}",
                    session_name, error_message
                );
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
        "Session '{}': expected to receive an error, but none was received",
        session_name
    );

    assert!(
        error_message
            .to_lowercase()
            .contains(&expected_text.to_lowercase()),
        "Session '{}': expected error to contain '{}', got '{}'",
        session_name,
        expected_text,
        error_message
    );
}

#[then(regex = r#"^session "([^"]+)" should complete without error$"#)]
pub async fn session_should_complete_without_error(world: &mut DoormanWorld, session_name: String) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

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
                eprintln!(
                    "Session '{}': received unexpected error: {}",
                    session_name, error_message
                );
            }
            'Z' => {
                // ReadyForQuery - done
                eprintln!("Session '{}': query completed successfully", session_name);
                break;
            }
            _ => {
                // Other messages - continue (T=RowDescription, D=DataRow, C=CommandComplete, etc.)
            }
        }
    }

    assert!(
        !error_found,
        "Session '{}': expected query to complete without error, but got: {}",
        session_name, error_message
    );
}
