use crate::cmd_args::GenerateConfig;
use crate::config::{Config, PoolMode};
use crate::generate::generate_config_with_client;
use mockall::predicate::*;
use mockall::*;
use postgres::error::{DbError, ErrorPosition};
use postgres::types::Type;
use postgres::{Client, Error as PgError, GenericClient, Row};
use std::collections::HashMap;
use std::error::Error;

// Mock for PostgreSQL Client
mock! {
    pub PostgresClient {}

    impl GenericClient for PostgresClient {
        type Row = MockRow;
        type Portal = postgres::Portal<Self::Row>;
        type CopyIn = postgres::CopyIn;
        type CopyOut = postgres::CopyOut;

        fn query(&mut self, query: &str, params: &[&(dyn postgres::types::ToSql + Sync)]) -> Result<Vec<Self::Row>, PgError>;
        
        fn query_one(&mut self, _query: &str, _params: &[&(dyn postgres::types::ToSql + Sync)]) -> Result<Self::Row, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn query_opt(&mut self, _query: &str, _params: &[&(dyn postgres::types::ToSql + Sync)]) -> Result<Option<Self::Row>, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn prepare(&mut self, _query: &str) -> Result<postgres::Statement<Self::Row>, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn prepare_typed(&mut self, _query: &str, _types: &[Type]) -> Result<postgres::Statement<Self::Row>, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn execute(&mut self, _query: &str, _params: &[&(dyn postgres::types::ToSql + Sync)]) -> Result<u64, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn execute_iter(&mut self, _query: &str, _params: &[&(dyn postgres::types::ToSql + Sync)]) -> Result<Self::Portal, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn copy_in(&mut self, _query: &str) -> Result<Self::CopyIn, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn copy_out(&mut self, _query: &str) -> Result<Self::CopyOut, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn transaction(&mut self) -> Result<postgres::Transaction<'_>, PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn batch_execute(&mut self, _query: &str) -> Result<(), PgError> {
            unimplemented!("Not needed for this test")
        }
        
        fn is_closed(&self) -> bool {
            false
        }
    }
}

// Mock for PostgreSQL Row
#[derive(Clone)]
struct MockRow {
    values: HashMap<String, String>,
}

impl MockRow {
    fn new(values: HashMap<String, String>) -> Self {
        Self { values }
    }
}

impl postgres::Row for MockRow {
    fn get<T: postgres::types::FromSql<'_>>(&self, index: usize) -> Result<T, Box<dyn Error + Sync + Send>> {
        // This is a simplified implementation that only handles String type
        // In a real implementation, you would need to handle different types
        let column_name = match index {
            0 => "usename",
            1 => "passwd",
            _ => "datname",
        };
        
        if let Some(value) = self.values.get(column_name) {
            // This is a hack to convert String to T
            // In a real implementation, you would need to handle different types properly
            let value_any = value as &dyn std::any::Any;
            if let Some(s) = value_any.downcast_ref::<String>() {
                // Try to convert the string to T
                // This will only work for String type
                let boxed: Box<dyn std::any::Any> = Box::new(s.clone());
                return Ok(*boxed.downcast::<T>().unwrap());
            }
        }
        
        Err("Type conversion error".into())
    }

    fn columns(&self) -> &[postgres::Column] {
        unimplemented!("Not needed for this test")
    }

    fn len(&self) -> usize {
        self.values.len()
    }
}

// Helper function to create a mock client with predefined responses
fn create_mock_client() -> MockPostgresClient {
    let mut client = MockPostgresClient::new();
    
    // Mock response for pg_shadow query
    let user_rows = vec![
        MockRow::new(HashMap::from([
            ("usename".to_string(), "postgres".to_string()),
            ("passwd".to_string(), "md5abcdef1234567890".to_string()),
        ])),
        MockRow::new(HashMap::from([
            ("usename".to_string(), "testuser".to_string()),
            ("passwd".to_string(), "md5fedcba0987654321".to_string()),
        ])),
    ];
    
    // Mock response for pg_database query
    let db_rows = vec![
        MockRow::new(HashMap::from([
            ("datname".to_string(), "postgres".to_string()),
        ])),
        MockRow::new(HashMap::from([
            ("datname".to_string(), "testdb".to_string()),
        ])),
    ];
    
    // Set up expectations for the pg_shadow query
    client
        .expect_query()
        .with(eq("SELECT usename, passwd FROM pg_shadow WHERE passwd is not null"), eq(&[] as &[&dyn postgres::types::ToSql]))
        .times(1)
        .returning(move |_, _| Ok(user_rows.clone()));
    
    // Set up expectations for the pg_database query
    client
        .expect_query()
        .with(eq("SELECT datname FROM pg_database WHERE not datistemplate"), eq(&[] as &[&dyn postgres::types::ToSql]))
        .times(1)
        .returning(move |_, _| Ok(db_rows.clone()));
    
    client
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    
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
        };
        
        // Create a mock client
        let mut client = create_mock_client();
        
        // Call the function with our mock client
        let result = generate_config_with_client(&config, &mut client);
        
        // Verify the result
        assert!(result.is_ok());
        
        let config_result = result.unwrap();
        
        // Verify the configuration has the expected values
        assert_eq!(config_result.general.host, "localhost");
        assert_eq!(config_result.general.port, 6432);
        assert_eq!(config_result.general.server_tls, false);
        
        // Verify the pools
        assert_eq!(config_result.pools.len(), 2);
        assert!(config_result.pools.contains_key("postgres"));
        assert!(config_result.pools.contains_key("testdb"));
        
        // Verify the users in the pools
        let postgres_pool = config_result.pools.get("postgres").unwrap();
        assert_eq!(postgres_pool.pool_mode, PoolMode::Transaction);
        assert_eq!(postgres_pool.users.len(), 2);
        assert!(postgres_pool.users.contains_key("postgres"));
        assert!(postgres_pool.users.contains_key("testuser"));
        
        // Verify user details
        let postgres_user = postgres_pool.users.get("postgres").unwrap();
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
        };
        
        // Create a mock client
        let mut client = create_mock_client();
        
        // Call the function with our mock client
        let result = generate_config_with_client(&config, &mut client);
        
        // Verify the result
        assert!(result.is_ok());
        
        let config_result = result.unwrap();
        
        // Verify the configuration has the expected values
        assert_eq!(config_result.general.host, "testhost");
        assert_eq!(config_result.general.port, 6432);
        assert_eq!(config_result.general.server_tls, false);
        
        // Verify the pools
        assert_eq!(config_result.pools.len(), 2);
        
        // Verify the pool mode is Session as specified
        let testdb_pool = config_result.pools.get("testdb").unwrap();
        assert_eq!(testdb_pool.pool_mode, PoolMode::Session);
        assert_eq!(testdb_pool.server_host, "testhost");
        assert_eq!(testdb_pool.server_port, 5433);
        
        // Verify user details
        let testuser = testdb_pool.users.get("testuser").unwrap();
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
        };
        
        // Create a mock client
        let mut client = create_mock_client();
        
        // Call the function with our mock client
        let result = generate_config_with_client(&config, &mut client);
        
        // Verify the result
        assert!(result.is_ok());
        
        let config_result = result.unwrap();
        
        // Verify SSL is enabled
        assert_eq!(config_result.general.server_tls, true);
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
        };
        
        // Create a mock client that returns an error
        let mut client = MockPostgresClient::new();
        
        // Set up expectations for the pg_shadow query to return an error
        client
            .expect_query()
            .with(eq("SELECT usename, passwd FROM pg_shadow WHERE passwd is not null"), eq(&[] as &[&dyn postgres::types::ToSql]))
            .times(1)
            .returning(|_, _| {
                Err(PgError::new(
                    "permission denied for table pg_shadow",
                    Some("42501".to_string()),
                ))
            });
        
        // Call the function with our mock client
        let result = generate_config_with_client(&config, &mut client);
        
        // Verify the result is an error
        assert!(result.is_err());
    }
}