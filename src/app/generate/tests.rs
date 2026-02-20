use super::*;

// Test-specific implementation of generate_config_with_client
// This is used by the `#[cfg(test)]` `generate_config` wrapper.
pub fn generate_config_with_client<
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
>(
    config: &GenerateConfig,
    users: Result<Vec<(String, String)>, E1>,
    databases: Result<Vec<String>, E2>,
) -> Result<Config, Box<dyn Error>> {
    // Initialize default configuration
    let mut result = Config::default();
    result.general.host = config.host.as_deref().unwrap_or("localhost").to_string();
    result.general.port = 6432; // Default port for pg_doorman
    result.general.server_tls = config.ssl;

    // Store users with their authentication details
    let mut users_vec = Vec::new();

    // Process users if available
    match users {
        Ok(user_list) => {
            for (username, password) in user_list {
                // Create user configuration for each PostgreSQL user
                let user = crate::config::User {
                    username,
                    password,
                    pool_size: config.pool_size,
                    min_pool_size: None,
                    pool_mode: None,
                    server_lifetime: None,
                    server_username: None,
                    server_password: None,
                    auth_pam_service: None,
                };
                users_vec.push(user);
            }
        }
        Err(e) => return Err(Box::new(e)),
    }

    // Process databases if available
    match databases {
        Ok(db_list) => {
            for db_name in db_list {
                // Determine pool mode based on configuration
                let pool_mode = if config.session_pool_mode {
                    PoolMode::Session
                } else {
                    PoolMode::Transaction
                };

                // Add database to configuration with all discovered users
                result.pools.insert(
                    db_name.clone(),
                    crate::config::Pool {
                        pool_mode,
                        connect_timeout: None,
                        idle_timeout: None,
                        server_lifetime: None,
                        cleanup_server_connections: false,
                        log_client_parameter_status_changes: false,
                        application_name: None,
                        server_host: config
                            .server_host
                            .as_deref()
                            .unwrap_or(config.host.as_deref().unwrap_or("localhost"))
                            .to_string(),
                        server_port: config.port,
                        server_database: Some(db_name.to_string()),
                        prepared_statements_cache_size: None,
                        users: users_vec.clone(),
                    },
                );
            }
        }
        Err(e) => return Err(Box::new(e)),
    }

    result.path = "".to_string();
    Ok(result)
}

#[test]
fn test_generate_config_with_default_parameters() {
    // Create a GenerateConfig with default parameters
    let config = GenerateConfig {
        host: None,
        port: 5432,
        user: None,
        password: None,
        database: None,
        ssl: false,
        pool_size: 40,
        session_pool_mode: false,
        output: None,
        server_host: None,
        no_comments: false,
        reference: false,
        russian_comments: false,
        format: None,
    };

    // Create mock data
    let users = vec![
        ("postgres".to_string(), "md5abcdef1234567890".to_string()),
        ("testuser".to_string(), "md5fedcba0987654321".to_string()),
    ];

    let databases = vec!["postgres".to_string(), "testdb".to_string()];

    // Call the function with our mock data and explicit error types
    let result = generate_config_with_client::<std::convert::Infallible, std::convert::Infallible>(
        &config,
        Ok(users),
        Ok(databases),
    );

    // Verify the result
    assert!(result.is_ok());

    let config_result = result.unwrap();

    // Verify the configuration has the expected values
    assert_eq!(config_result.general.host, "localhost");
    assert_eq!(config_result.general.port, 6432);
    assert!(!config_result.general.server_tls);

    // Verify the pools
    assert_eq!(config_result.pools.len(), 2);
    assert!(config_result.pools.contains_key("postgres"));
    assert!(config_result.pools.contains_key("testdb"));

    // Verify the users in the pools
    let postgres_pool = config_result.pools.get("postgres").unwrap();
    assert_eq!(postgres_pool.pool_mode, PoolMode::Transaction);
    assert_eq!(postgres_pool.users.len(), 2);
    assert!(postgres_pool.users.iter().any(|u| u.username == "postgres"));
    assert!(postgres_pool.users.iter().any(|u| u.username == "testuser"));

    // Verify user details
    let postgres_user = postgres_pool
        .users
        .iter()
        .find(|u| u.username == "postgres")
        .unwrap();
    assert_eq!(postgres_user.username, "postgres");
    assert_eq!(postgres_user.password, "md5abcdef1234567890");
    assert_eq!(postgres_user.pool_size, 40);
}

#[test]
fn test_generate_config_with_custom_parameters() {
    // Create a GenerateConfig with custom parameters
    let config = GenerateConfig {
        host: Some("testhost".to_string()),
        port: 5433,
        user: Some("testuser".to_string()),
        password: Some("testpass".to_string()),
        database: Some("testdb".to_string()),
        ssl: false,
        pool_size: 20,
        session_pool_mode: true,
        output: None,
        server_host: None,
        no_comments: false,
        reference: false,
        russian_comments: false,
        format: None,
    };

    // Create mock data
    let users = vec![
        ("postgres".to_string(), "md5abcdef1234567890".to_string()),
        ("testuser".to_string(), "md5fedcba0987654321".to_string()),
    ];

    let databases = vec!["postgres".to_string(), "testdb".to_string()];

    // Call the function with our mock data and explicit error types
    let result = generate_config_with_client::<std::convert::Infallible, std::convert::Infallible>(
        &config,
        Ok(users),
        Ok(databases),
    );

    // Verify the result
    assert!(result.is_ok());

    let config_result = result.unwrap();

    // Verify the configuration has the expected values
    assert_eq!(config_result.general.host, "testhost");
    assert_eq!(config_result.general.port, 6432);
    assert!(!config_result.general.server_tls);

    // Verify the pools
    assert_eq!(config_result.pools.len(), 2);

    // Verify the pool mode is Session as specified
    let testdb_pool = config_result.pools.get("testdb").unwrap();
    assert_eq!(testdb_pool.pool_mode, PoolMode::Session);
    assert_eq!(testdb_pool.server_host, "testhost");
    assert_eq!(testdb_pool.server_port, 5433);

    // Verify user details
    let testuser = testdb_pool
        .users
        .iter()
        .find(|u| u.username == "testuser")
        .unwrap();
    assert_eq!(testuser.username, "testuser");
    assert_eq!(testuser.pool_size, 20);
}

#[test]
fn test_generate_config_with_ssl_enabled() {
    // Create a GenerateConfig with SSL enabled
    let config = GenerateConfig {
        host: None,
        port: 5432,
        user: None,
        password: None,
        database: None,
        ssl: true,
        pool_size: 40,
        session_pool_mode: false,
        output: None,
        server_host: None,
        no_comments: false,
        reference: false,
        russian_comments: false,
        format: None,
    };

    // Create mock data
    let users = vec![
        ("postgres".to_string(), "md5abcdef1234567890".to_string()),
        ("testuser".to_string(), "md5fedcba0987654321".to_string()),
    ];

    let databases = vec!["postgres".to_string(), "testdb".to_string()];

    // Call the function with our mock data and explicit error types
    let result = generate_config_with_client::<std::convert::Infallible, std::convert::Infallible>(
        &config,
        Ok(users),
        Ok(databases),
    );

    // Verify the result
    assert!(result.is_ok());

    let config_result = result.unwrap();

    // Verify SSL is enabled
    assert!(config_result.general.server_tls);
}

#[test]
fn test_generate_config_with_database_error() {
    // Create a GenerateConfig
    let config = GenerateConfig {
        host: None,
        port: 5432,
        user: None,
        password: None,
        database: None,
        ssl: false,
        pool_size: 40,
        session_pool_mode: false,
        output: None,
        server_host: None,
        no_comments: false,
        reference: false,
        russian_comments: false,
        format: None,
    };

    // Create a simple error type for testing
    #[derive(Debug)]
    struct TestError(String);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for TestError {}

    // Create an error directly instead of using postgres::Error
    let error = TestError("permission denied for table pg_shadow".to_string());

    let databases = vec!["postgres".to_string(), "testdb".to_string()];

    // Call the function with our mock data including the error and explicit error types
    let result = generate_config_with_client::<TestError, std::convert::Infallible>(
        &config,
        Err(error),
        Ok(databases),
    );

    // Verify the result is an error
    assert!(result.is_err());
}
