use crate::fuzz_client::FuzzClient;
use crate::world::DoormanWorld;
use cucumber::{given, then, when};

// Default credentials for fuzz tests
const FUZZ_USER: &str = "example_user_1";
const FUZZ_DB: &str = "example_db";

/// Get the doorman address from world
fn get_doorman_addr(world: &DoormanWorld) -> String {
    let port = world
        .doorman_port
        .expect("pg_doorman port not set - start pg_doorman first");
    format!("127.0.0.1:{}", port)
}

// ============================================================================
// Broken Headers - all methods connect and authenticate FIRST
// ============================================================================

#[when("fuzzer connects and sends broken length header")]
pub async fn fuzzer_sends_broken_length_header(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_broken_length_header(FUZZ_USER, FUZZ_DB).await;
}

#[when("fuzzer connects and sends negative length")]
pub async fn fuzzer_sends_negative_length(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_negative_length(FUZZ_USER, FUZZ_DB).await;
}

#[when("fuzzer connects and sends truncated message")]
pub async fn fuzzer_sends_truncated_message(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_truncated_message(FUZZ_USER, FUZZ_DB).await;
}

// ============================================================================
// Invalid Message Types - all methods connect and authenticate FIRST
// ============================================================================

#[when(regex = r"^fuzzer connects and sends unknown message type '(.)'$")]
pub async fn fuzzer_sends_unknown_message_type(world: &mut DoormanWorld, msg_type: char) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client
        .send_unknown_message_type(FUZZ_USER, FUZZ_DB, msg_type as u8)
        .await;
}

#[when("fuzzer connects and sends server-only message type 'T'")]
pub async fn fuzzer_sends_server_message_type(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_server_message_type(FUZZ_USER, FUZZ_DB).await;
}

#[when("fuzzer connects and sends null byte message type")]
pub async fn fuzzer_sends_null_message_type(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_null_message_type(FUZZ_USER, FUZZ_DB).await;
}

// ============================================================================
// Gigantic Messages - connects and authenticates FIRST
// ============================================================================

#[when("fuzzer connects and sends message with 256MB length claim")]
pub async fn fuzzer_sends_gigantic_length(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_gigantic_length(FUZZ_USER, FUZZ_DB).await;
}

// ============================================================================
// Protocol Violations - connects and authenticates FIRST
// ============================================================================

#[when("fuzzer connects, authenticates, and sends Execute without Bind")]
pub async fn fuzzer_sends_execute_without_bind(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_execute_without_bind(FUZZ_USER, FUZZ_DB).await;
}

#[when("fuzzer connects, authenticates, and sends Bind to nonexistent statement")]
pub async fn fuzzer_sends_bind_nonexistent(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_bind_nonexistent(FUZZ_USER, FUZZ_DB).await;
}

// ============================================================================
// Random Attacks - all attacks connect and authenticate FIRST
// ============================================================================

#[when(regex = r"^fuzzer attacks with (\d+) random malformed connections$")]
pub async fn fuzzer_attacks_random(world: &mut DoormanWorld, count: usize) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.attack_random(FUZZ_USER, FUZZ_DB, count).await;
}

// ============================================================================
// Verification Steps
// ============================================================================

#[then("pg_doorman should still be running")]
pub async fn verify_doorman_running(world: &mut DoormanWorld) {
    // Check if the process is still alive
    if let Some(ref mut child) = world.doorman_process {
        match child.try_wait() {
            Ok(Some(status)) => {
                panic!("pg_doorman crashed with status: {:?}", status);
            }
            Ok(None) => {
                // Process is still running - good!
            }
            Err(e) => {
                panic!("Error checking pg_doorman process: {:?}", e);
            }
        }
    } else {
        panic!("pg_doorman process not found in world");
    }
}

#[given("fuzzer sends multiple broken headers in parallel")]
pub async fn fuzzer_sends_parallel_broken_headers(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);

    // Spawn multiple concurrent fuzz attacks - all connect and authenticate FIRST
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let addr = addr.clone();
            tokio::spawn(async move {
                let client = FuzzClient::new(&addr);
                let _ = client.send_broken_length_header(FUZZ_USER, FUZZ_DB).await;
                let _ = client.send_negative_length(FUZZ_USER, FUZZ_DB).await;
                let _ = client.send_truncated_message(FUZZ_USER, FUZZ_DB).await;
            })
        })
        .collect();

    // Wait for all to complete
    for handle in handles {
        let _ = handle.await;
    }
}

#[when("fuzzer sends random garbage data")]
pub async fn fuzzer_sends_random_garbage(world: &mut DoormanWorld) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_random_garbage(FUZZ_USER, FUZZ_DB, 1000).await;
}

#[when(regex = r"^fuzzer connects and sends (\d+) bytes of random data$")]
pub async fn fuzzer_sends_n_bytes_random(world: &mut DoormanWorld, size: usize) {
    let addr = get_doorman_addr(world);
    let client = FuzzClient::new(&addr);
    let _ = client.send_random_garbage(FUZZ_USER, FUZZ_DB, size).await;
}
