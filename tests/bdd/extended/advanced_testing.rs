use crate::world::DoormanWorld;
use cucumber::{then, when};

#[when(regex = r#"^we read (\d+) bytes from session "([^"]+)"$"#)]
pub async fn read_bytes_from_session(world: &mut DoormanWorld, bytes: usize, session_name: String) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let bytes_read = conn
        .read_limited_bytes(bytes)
        .await
        .expect("Failed to read bytes from session");

    eprintln!(
        "Session '{}': read {} bytes (requested {})",
        session_name, bytes_read, bytes
    );
}

#[when(regex = r#"^we send SimpleQuery "([^"]+)" to session "([^"]+)" and verify no stale data$"#)]
pub async fn send_query_and_verify_no_stale_data(
    world: &mut DoormanWorld,
    query: String,
    session_name: String,
) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

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
                for field in super::helpers::parse_datarow_fields(&data) {
                    data_content.push_str(&field);
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
                panic!(
                    "Session '{}': received unexpected error: {}",
                    session_name, error_str
                );
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
        .unwrap_or_else(|| panic!("No messages stored for session '{}'", session_name));

    let data_content = if let Some((_, data)) = messages.first() {
        String::from_utf8_lossy(data).to_string()
    } else {
        String::new()
    };

    // Verify that the response contains ONLY the expected marker
    // and no stale data (like 'X', 'A', 'B', 'C', 'T' repeated patterns from previous queries)
    assert!(
        data_content.contains(&expected_marker),
        "Session '{}': expected response to contain marker '{}', got '{}'",
        session_name,
        expected_marker,
        data_content
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

    eprintln!(
        "Session '{}': verified clean response with marker '{}'",
        session_name, expected_marker
    );
}

// =============================================================================
// Flush timeout protocol violation test steps
// =============================================================================

/// Send Sync to a session without waiting for the server response.
/// Used when we expect the server roundtrip to timeout (e.g., PostgreSQL is frozen).
#[when(regex = r#"^we send Sync to session "([^"]+)" without waiting for response$"#)]
pub async fn send_sync_to_session_no_wait(world: &mut DoormanWorld, session_name: String) {
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    conn.send_sync().await.expect("Failed to send Sync");
    eprintln!(
        "Session '{}': Sync sent (not waiting for response)",
        session_name
    );
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
        .unwrap_or_else(|e| panic!("Failed to read postmaster.pid at {:?}: {}", pid_file, e));
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
    eprintln!(
        "Freezing PostgreSQL: finding all child processes of postmaster PID {}",
        pid
    );

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
        eprintln!("Sending SIGSTOP to PG process {}", p);
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
        .unwrap_or_else(|e| panic!("Failed to read postmaster.pid at {:?}: {}", pid_file, e));
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
        eprintln!("Sending SIGCONT to PG process {}", p);
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
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    // Build a large SQL query padded with comments
    let base_query = "SELECT 1";
    let padding_size = query_kb * 1024 - base_query.len() - 6; // 6 for "/* */"
    let padding: String = "x".repeat(padding_size);
    let large_query = format!("{}/* {} */", base_query, padding);

    eprintln!(
        "Sending {} Parse messages (~{}KB each, ~{}MB total) to session '{}'",
        count,
        query_kb,
        (count * query_kb) / 1024,
        session_name
    );

    for i in 0..count {
        let stmt_name = format!("flush_test_{}", i);
        if let Err(e) = conn.send_parse(&stmt_name, &large_query).await {
            eprintln!(
                "Write failed at message {} (expected after buffer fills): {}",
                i, e
            );
            break;
        }
    }

    eprintln!("Finished sending batch to session '{}'", session_name);
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
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

    let base_query = "SELECT 1";
    let padding_size = size_kb * 1024 - base_query.len() - 6; // 6 for "/* */"
    let padding: String = "x".repeat(padding_size);
    let large_query = format!("{}/* {} */", base_query, padding);

    eprintln!(
        "Sending large SimpleQuery (~{}KB) to session '{}'",
        size_kb, session_name
    );

    if let Err(e) = conn.send_simple_query(&large_query).await {
        eprintln!(
            "Write failed for large SimpleQuery (expected after buffer fills): {}",
            e
        );
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
    let conn = super::helpers::get_session(&mut world.named_sessions, &session_name);

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
                eprintln!("Session '{}': received message {}", session_name, desc);
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
                    "Session '{}': connection error (this is the bug - no ErrorResponse sent): {}",
                    session_name, e
                );
                got_eof = true;
                break;
            }
            Err(_) => {
                // Timeout - no more messages
                eprintln!(
                    "Session '{}': read timeout (no more messages)",
                    session_name
                );
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

    eprintln!(
        "Session '{}': correctly received ErrorResponse on flush timeout",
        session_name
    );
}
