use std::collections::BTreeMap;
use std::error::Error;

use crate::app::args::GenerateConfig;
use crate::config::{Config, PoolMode};

#[cfg(not(test))]
use native_tls::TlsConnector;
#[cfg(not(test))]
use postgres::{Client, NoTls};
#[cfg(not(test))]
use postgres_native_tls::MakeTlsConnector;

#[cfg(not(test))]
/// Generates a pg_doorman configuration based on provided settings
/// Automatically detects users and databases from the PostgreSQL instance
pub fn generate_config(config: &GenerateConfig) -> Result<Config, Box<dyn Error>> {
    // Initialize default configuration
    let mut result = Config::default();
    result.general.host = config.host.as_deref().unwrap_or("localhost").to_string();
    result.general.port = 6432; // Default port for pg_doorman
    result.general.server_tls = config.ssl;

    // Create connection string from the provided configuration
    let connection_string = format!(
        "host={} port={} user={} password={} dbname={}",
        config.host.as_deref().unwrap_or("localhost"),
        config.port,
        config.user.as_deref().unwrap_or("postgres"),
        config.password.as_deref().unwrap_or(""),
        config.database.as_deref().unwrap_or("postgres")
    );

    // Connect to the PostgreSQL database
    // Use TLS if SSL is enabled in configuration
    let client = if config.ssl {
        let connector = TlsConnector::builder().build()?;
        let connector = MakeTlsConnector::new(connector);
        Client::connect(&connection_string, connector)?
    } else {
        Client::connect(&connection_string, NoTls)?
    };

    // Call the internal function with the created client
    generate_config_with_client(config, client)
}

#[cfg(test)]
/// Test version of generate_config that uses mock data
pub fn generate_config(config: &GenerateConfig) -> Result<Config, Box<dyn Error>> {
    // Create mock data for testing
    let users = vec![
        ("postgres".to_string(), "md5abcdef1234567890".to_string()),
        ("testuser".to_string(), "md5fedcba0987654321".to_string()),
    ];

    let databases = vec!["postgres".to_string(), "testdb".to_string()];

    // Call the test-specific implementation with explicit error types
    tests::generate_config_with_client::<std::convert::Infallible, std::convert::Infallible>(
        config,
        Ok(users),
        Ok(databases),
    )
}

#[cfg(not(test))]
/// Internal function that accepts a client for testing purposes
/// This allows us to inject a mock client in tests
pub fn generate_config_with_client(
    config: &GenerateConfig,
    mut client: Client,
) -> Result<Config, Box<dyn Error>> {
    // Initialize default configuration
    let mut result = Config::default();
    result.general.host = "0.0.0.0".to_string();
    result.general.port = 6432; // Default port for pg_doorman
    result.general.server_tls = config.ssl;

    // Store users with their authentication details
    let mut users = BTreeMap::new();
    {
        // Query pg_shadow to get username and password hashes (requires superuser privileges)
        let rows = client.query(
            "SELECT usename, passwd FROM pg_shadow WHERE passwd is not null",
            &[],
        )?;
        for row in rows {
            let usename: String = row.get(0);
            let passwd: String = row.get(1);
            // Create user configuration for each PostgreSQL user
            let user = crate::config::User {
                username: usename.clone(),
                password: passwd.clone(),
                pool_size: config.pool_size,
                min_pool_size: None,
                pool_mode: None,
                server_lifetime: None,
                server_username: None,
                server_password: None,
                auth_pam_service: None,
            };
            users.insert(usename, user);
        }
    }

    {
        // Query pg_database to get all non-template databases
        let rows = client.query(
            "SELECT datname FROM pg_database WHERE not datistemplate",
            &[],
        )?;
        for row in rows {
            // Determine pool mode based on configuration
            let pool_mode = if config.session_pool_mode {
                PoolMode::Session
            } else {
                PoolMode::Transaction
            };
            let datname: String = row.get(0);
            // Add database to configuration with all discovered users
            result.pools.insert(
                datname.clone(),
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
                    server_database: Some(datname.to_string()),
                    prepared_statements_cache_size: None,
                    users: users.clone(),
                },
            );
        }
    };
    result.path = "".to_string();
    Ok(result)
}

#[cfg(test)]
mod tests;
