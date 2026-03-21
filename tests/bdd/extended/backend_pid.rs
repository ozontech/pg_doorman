use crate::world::DoormanWorld;
use cucumber::{then, when};

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
            panic!(
                "Named backend PID '{}' for session '{}' not found",
                pid_name1, session_name
            )
        });
    let pid2 = world
        .named_backend_pids
        .get(&(session_name.clone(), pid_name2.clone()))
        .unwrap_or_else(|| {
            panic!(
                "Named backend PID '{}' for session '{}' not found",
                pid_name2, session_name
            )
        });

    println!(
        "Session '{}' backend_pid '{}': {}",
        session_name, pid_name1, pid1
    );
    println!(
        "Session '{}' backend_pid '{}': {}",
        session_name, pid_name2, pid2
    );

    assert_ne!(
        pid1, pid2,
        "Backend PIDs should be different: '{}' ({}) vs '{}' ({})",
        pid_name1, pid1, pid_name2, pid2
    );
}

#[then(regex = r#"^named backend_pid "([^"]+)" from session "([^"]+)" is same as "([^"]+)"$"#)]
pub async fn compare_named_backend_pids_same(
    world: &mut DoormanWorld,
    pid_name1: String,
    session_name: String,
    pid_name2: String,
) {
    let pid1 = world
        .named_backend_pids
        .get(&(session_name.clone(), pid_name1.clone()))
        .unwrap_or_else(|| {
            panic!(
                "Named backend PID '{}' for session '{}' not found",
                pid_name1, session_name
            )
        });
    let pid2 = world
        .named_backend_pids
        .get(&(session_name.clone(), pid_name2.clone()))
        .unwrap_or_else(|| {
            panic!(
                "Named backend PID '{}' for session '{}' not found",
                pid_name2, session_name
            )
        });

    println!(
        "Session '{}' backend_pid '{}': {}",
        session_name, pid_name1, pid1
    );
    println!(
        "Session '{}' backend_pid '{}': {}",
        session_name, pid_name2, pid2
    );

    assert_eq!(
        pid1, pid2,
        "Backend PIDs should be the same: '{}' ({}) vs '{}' ({})",
        pid_name1, pid1, pid_name2, pid2
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
        .unwrap_or_else(|| panic!("Backend PID for session '{}' not found", target_session));

    let terminate_query = format!("SELECT pg_terminate_backend({})", backend_pid);
    eprintln!(
        "Terminating backend of session '{}' (pid={}) via session '{}'",
        target_session, backend_pid, killer_session
    );

    // Get killer session connection
    let conn = super::helpers::get_session(&mut world.named_sessions, &killer_session);

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
            panic!(
                "Named backend PID '{}' from session '{}' not found",
                pid_name, source_session
            )
        });

    let terminate_query = format!("SELECT pg_terminate_backend({})", backend_pid);
    eprintln!(
        "Terminating named backend '{}' from session '{}' (pid={}) via session '{}'",
        pid_name, source_session, backend_pid, killer_session
    );

    // Get killer session connection
    let conn = super::helpers::get_session(&mut world.named_sessions, &killer_session);

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
