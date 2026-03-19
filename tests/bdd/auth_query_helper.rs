//! BDD step definitions for AuthQueryExecutor integration tests.
//!
//! Tests the executor against a real PostgreSQL instance using a custom
//! auth_users table that simulates pg_shadow.

use cucumber::{given, then, when};

use pg_doorman::auth::auth_query::AuthQueryExecutor;
use pg_doorman::config::{AuthQueryConfig, Duration};
use pg_doorman::errors::Error;

use crate::world::DoormanWorld;

/// Build an AuthQueryConfig for tests with sensible defaults.
fn build_test_config(query: &str, pool_size: u32, database: Option<String>) -> AuthQueryConfig {
    AuthQueryConfig {
        query: query.to_string(),
        user: "postgres".to_string(),
        password: String::new(), // trust mode
        database,
        credential_lookup_pool_size: pool_size,
        server_user: None,
        server_password: None,
        pool_size: 40,
        min_pool_size: 0,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    }
}

#[given(regex = r#"^auth_query executor connected with query "([^"]+)" and pool_size (\d+)$"#)]
async fn create_executor(world: &mut DoormanWorld, query: String, pool_size: u32) {
    let pg_port = world.pg_port.expect("PostgreSQL must be running");
    let config = build_test_config(&query, pool_size, None);

    let executor = AuthQueryExecutor::new(&config, "postgres", "127.0.0.1", pg_port)
        .await
        .expect("Failed to create AuthQueryExecutor");

    world.auth_query_executor = Some(executor);
}

#[given(
    regex = r#"^auth_query executor connected to database "([^"]+)" with query "([^"]+)" and pool_size (\d+)$"#
)]
async fn create_executor_with_database(
    world: &mut DoormanWorld,
    database: String,
    query: String,
    pool_size: u32,
) {
    let pg_port = world.pg_port.expect("PostgreSQL must be running");
    let config = build_test_config(&query, pool_size, Some(database));

    let executor = AuthQueryExecutor::new(&config, "postgres", "127.0.0.1", pg_port)
        .await
        .expect("Failed to create AuthQueryExecutor");

    world.auth_query_executor = Some(executor);
}

#[then(
    regex = r#"^auth_query executor creation should fail for host "([^"]+)" port (\d+) with connection error$"#
)]
async fn executor_creation_should_fail(world: &mut DoormanWorld, host: String, port: u16) {
    let _ = world; // suppress unused warning (world is required by cucumber signature)
    let config = build_test_config("SELECT $1::text, 'hash'", 1, Some("postgres".to_string()));

    let result = AuthQueryExecutor::new(&config, "test_pool", &host, port).await;

    match result {
        Err(Error::AuthQueryConnectionError(_)) => {
            // Expected: server unreachable
        }
        Err(e) => {
            panic!("Expected AuthQueryConnectionError, got: {:?}", e);
        }
        Ok(_) => {
            panic!("Expected executor creation to fail, but it succeeded");
        }
    }
}

#[when(regex = r#"^auth_query fetches password for user "([^"]+)"$"#)]
async fn fetch_password(world: &mut DoormanWorld, username: String) {
    let executor = world
        .auth_query_executor
        .as_ref()
        .expect("AuthQueryExecutor not created — missing Given step");

    let result = executor.fetch_password(&username).await;
    world.auth_query_last_result = Some(result);
}

#[then(regex = r#"^auth_query result should contain user "([^"]+)" with password "([^"]+)"$"#)]
async fn result_should_contain_exact(
    world: &mut DoormanWorld,
    _expected_user: String,
    expected_password: String,
) {
    let result = world
        .auth_query_last_result
        .as_ref()
        .expect("No auth_query result — missing When step");

    match result {
        Ok(Some(password)) => {
            assert_eq!(password, &expected_password, "Password mismatch");
        }
        Ok(None) => panic!("Expected password found, got None (not found)"),
        Err(e) => panic!("Expected success, got error: {}", e),
    }
}

#[then(
    regex = r#"^auth_query result should contain user "([^"]+)" with password starting with "([^"]+)"$"#
)]
async fn result_should_contain_password_prefix(
    world: &mut DoormanWorld,
    _expected_user: String,
    password_prefix: String,
) {
    let result = world
        .auth_query_last_result
        .as_ref()
        .expect("No auth_query result — missing When step");

    match result {
        Ok(Some(password)) => {
            assert!(
                password.starts_with(&password_prefix),
                "Password '{}' does not start with '{}'",
                password,
                password_prefix
            );
        }
        Ok(None) => panic!("Expected password found, got None (not found)"),
        Err(e) => panic!("Expected success, got error: {}", e),
    }
}

#[then(regex = r#"^auth_query result should contain user "([^"]+)"$"#)]
async fn result_should_contain_user(world: &mut DoormanWorld, _expected_user: String) {
    let result = world
        .auth_query_last_result
        .as_ref()
        .expect("No auth_query result — missing When step");

    match result {
        Ok(Some(_)) => {
            // User found — password returned
        }
        Ok(None) => panic!("Expected user found, got None (not found)"),
        Err(e) => panic!("Expected success, got error: {}", e),
    }
}

#[then("auth_query result should be not found")]
async fn result_should_be_not_found(world: &mut DoormanWorld) {
    let result = world
        .auth_query_last_result
        .as_ref()
        .expect("No auth_query result — missing When step");

    match result {
        Ok(None) => {
            // Expected: user not found
        }
        Ok(Some(_)) => {
            panic!("Expected not found, but password was returned");
        }
        Err(e) => panic!("Expected not found (Ok(None)), got error: {}", e),
    }
}

#[then(regex = r#"^auth_query result should be config error containing "([^"]+)"$"#)]
async fn result_should_be_config_error(world: &mut DoormanWorld, message_part: String) {
    let result = world
        .auth_query_last_result
        .as_ref()
        .expect("No auth_query result — missing When step");

    match result {
        Err(Error::AuthQueryConfigError(msg)) => {
            assert!(
                msg.contains(&message_part),
                "Error message '{}' does not contain '{}'",
                msg,
                message_part
            );
        }
        Err(e) => {
            panic!(
                "Expected AuthQueryConfigError containing '{}', got: {}",
                message_part, e
            );
        }
        Ok(val) => panic!("Expected AuthQueryConfigError, got Ok({:?})", val.is_some()),
    }
}
