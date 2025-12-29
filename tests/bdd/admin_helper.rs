use crate::world::DoormanWorld;
use cucumber::{then, when};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_postgres::NoTls;

/// Start a background query through pg_doorman
#[when(expr = "a background query {string} is started as user {string} with password {string} to database {string}")]
pub async fn start_background_query(
    world: &mut DoormanWorld,
    query: String,
    user: String,
    password: String,
    database: String,
) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    
    let connection_string = format!(
        "host=127.0.0.1 port={} user={} password={} dbname={}",
        doorman_port, user, password, database
    );
    
    // Create a shared client holder for cancellation
    let client_holder: Arc<Mutex<Option<tokio_postgres::Client>>> = Arc::new(Mutex::new(None));
    let client_holder_clone = client_holder.clone();
    
    let query_clone = query.clone();
    let handle = tokio::spawn(async move {
        let (client, connection) = tokio_postgres::connect(&connection_string, NoTls)
            .await
            .expect("Failed to connect to pg_doorman");
        
        // Store the client for potential cancellation
        {
            let mut holder = client_holder_clone.lock().await;
            *holder = Some(client);
        }
        
        // Spawn the connection handler
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                // Connection errors are expected when we cancel
                eprintln!("Connection error (expected on cancel): {}", e);
            }
        });
        
        // Execute the long-running query
        let client_guard = client_holder_clone.lock().await;
        if let Some(ref client) = *client_guard {
            let _ = client.query(&query_clone, &[]).await;
        }
    });
    
    world.background_query_handle = Some(handle);
    world.background_query_client = Some(client_holder);
    
    // Give the query time to start
    sleep(Duration::from_millis(500)).await;
}

/// Check that PostgreSQL pg_stat_activity shows the query and save backend_pid
#[then(expr = "PostgreSQL pg_stat_activity shows the query {string}")]
pub async fn check_pg_stat_activity(world: &mut DoormanWorld, expected_query: String) {
    let pg_port = world.pg_port.expect("PostgreSQL not started");
    
    let connection_string = format!(
        "host=127.0.0.1 port={} user=postgres dbname=postgres",
        pg_port
    );
    
    let (client, connection) = tokio_postgres::connect(&connection_string, NoTls)
        .await
        .expect("Failed to connect to PostgreSQL");
    
    // Spawn the connection handler
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("PostgreSQL connection error: {}", e);
        }
    });
    
    let mut success = false;
    let mut found_pid: Option<i32> = None;
    
    for _ in 0..10 {
        let rows = client
            .query(
                "SELECT pid, query FROM pg_stat_activity WHERE state = 'active' AND query NOT LIKE '%pg_stat_activity%'",
                &[],
            )
            .await
            .expect("Failed to query pg_stat_activity");
        
        for row in rows {
            let query: &str = row.get(1);
            if query.contains(&expected_query) {
                let pid: i32 = row.get(0);
                found_pid = Some(pid);
                success = true;
                break;
            }
        }
        
        if success {
            break;
        }
        sleep(Duration::from_millis(500)).await;
    }
    
    assert!(success, "Query '{}' not found in pg_stat_activity", expected_query);
    
    // Save the backend_pid for later verification
    world.backend_pid = found_pid;
}

/// Check that pg_doorman admin console SHOW SERVERS shows the server_process_id in expected state
#[then(expr = "pg_doorman admin console {string} shows server_process_id in state {string}")]
pub async fn check_admin_console_show_servers(world: &mut DoormanWorld, command: String, expected_state: String) {
    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let expected_pid = world.backend_pid.expect("backend_pid not set - run pg_stat_activity check first");
    
    let connection_string = format!(
        "host=127.0.0.1 port={} user=admin password=admin dbname=pgbouncer",
        doorman_port
    );
    
    let mut success = false;
    let mut last_output = String::new();
    
    for _ in 0..10 {
        // Create a new connection for each attempt (admin console closes connection after query)
        let connect_result = tokio_postgres::connect(&connection_string, NoTls).await;
        
        let (client, connection) = match connect_result {
            Ok((c, conn)) => (c, conn),
            Err(e) => {
                eprintln!("Failed to connect to admin console: {}", e);
                sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        
        // Spawn the connection handler
        tokio::spawn(async move {
            let _ = connection.await;
        });
        
        // Use simple_query for admin console (it uses simple query protocol)
        let results = match client.simple_query(&command).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to execute admin command: {}", e);
                sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        
        // Build output for debugging and parse rows
        last_output.clear();
        for msg in &results {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = msg {
                let mut row_str = String::new();
                for i in 0..row.len() {
                    if let Some(val) = row.get(i) {
                        row_str.push_str(&format!("{} ", val));
                    }
                }
                last_output.push_str(&row_str);
                last_output.push('\n');
                
                // SHOW SERVERS columns:
                // 0: server_id, 1: server_process_id, 2: database_name, 3: user, 4: application_name,
                // 5: state, 6: wait, 7: transaction_count, 8: query_count, 9: bytes_sent,
                // 10: bytes_received, 11: age_seconds, 12: prepare_cache_hit, 13: prepare_cache_miss, 14: prepare_cache_size
                let mut row_state: Option<String> = None;
                let mut row_server_pid: Option<i32> = None;
                
                // server_process_id is column 1
                if let Some(pid_str) = row.get(1) {
                    if let Ok(pid) = pid_str.parse::<i32>() {
                        row_server_pid = Some(pid);
                    }
                }
                
                // state is column 5
                if let Some(state) = row.get(5) {
                    row_state = Some(state.to_string());
                }
                
                if let (Some(pid), Some(state)) = (row_server_pid, row_state) {
                    if pid == expected_pid && state == expected_state {
                        success = true;
                        break;
                    }
                }
            }
        }
        
        if success {
            break;
        }
        sleep(Duration::from_millis(500)).await;
    }
    
    assert!(
        success,
        "Server process ID {} in state '{}' not found in admin console output:\n{}",
        expected_pid,
        expected_state,
        last_output
    );
}

/// Cancel the background query
#[then("the background query is cancelled")]
pub async fn cancel_background_query(world: &mut DoormanWorld) {
    // Drop the client to close the connection and cancel the query
    if let Some(client_holder) = world.background_query_client.take() {
        let mut holder = client_holder.lock().await;
        *holder = None; // Drop the client
    }
    
    // Abort the task if still running
    if let Some(handle) = world.background_query_handle.take() {
        handle.abort();
        let _ = handle.await;
    }
}
