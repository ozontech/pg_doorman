//! Tests for configuration module.

use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

// Helper function to create a temporary config file for testing
fn create_temp_config() -> NamedTempFile {
    let config_content = r#"
[general]
host = "127.0.0.1"
port = 6432
admin_username = "admin"
admin_password = "admin_password"

[pools.example_db]
server_host = "localhost"
server_port = 5432
idle_timeout = 40000

[pools.example_db.users.0]
username = "example_user_1"
password = "password1"
pool_size = 40
pool_mode = "transaction"

[pools.example_db.users.1]
username = "example_user_2"
password = "SCRAM-SHA-256$4096:p2j/1lMdQF6r1dD9I9f7PQ==$H3xt5yh7lwSq9zUPYwHovRu3FyUCCXchG/skydJRa9o=:5xU6Wj/GNg3UnN2uQIx3ezx7uZyzGeM5NrvSJRIxnlw="
pool_size = 20

[pools.test_db1]
server_host = "localhost"
server_port = 5432

[pools.test_db2]
server_host = "localhost"
server_port = 5432

[pools.test_db3]
server_host = "localhost"
server_port = 5432
"#;
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();
    temp_file
}

#[tokio::test]
async fn test_config() {
    let temp_file = create_temp_config();
    let file_path = temp_file.path().to_str().unwrap();

    parse(file_path).await.unwrap();

    assert_eq!(get_config().pools.len(), 4);
    assert_eq!(get_config().pools["example_db"].idle_timeout, Some(40000));
    assert_eq!(
        get_config().pools["example_db"].users["0"].username,
        "example_user_1"
    );
    assert_eq!(
        get_config().pools["example_db"].users["1"].password,
        "SCRAM-SHA-256$4096:p2j/1lMdQF6r1dD9I9f7PQ==$H3xt5yh7lwSq9zUPYwHovRu3FyUCCXchG/skydJRa9o=:5xU6Wj/GNg3UnN2uQIx3ezx7uZyzGeM5NrvSJRIxnlw="
    );
    assert_eq!(get_config().pools["example_db"].users["1"].pool_size, 20);
    assert_eq!(
        get_config().pools["example_db"].users["1"].username,
        "example_user_2"
    );
    assert_eq!(get_config().pools["example_db"].users["0"].pool_size, 40);
    assert_eq!(
        get_config().pools["example_db"].users["0"].pool_mode,
        Some(PoolMode::Transaction)
    );
}

#[tokio::test]
async fn test_serialize_configs() {
    let temp_file = create_temp_config();
    let file_path = temp_file.path().to_str().unwrap();

    parse(file_path).await.unwrap();
    print!("{}", toml::to_string(&get_config()).unwrap());
}

// Tests for the validate function

// Test valid configuration
#[tokio::test]
async fn test_validate_valid_config() {
    let mut config = Config::default();

    // Add a pool with a user
    let mut pool = Pool::default();
    let user = User {
        username: "test_user".to_string(),
        password: "test_password".to_string(),
        pool_size: 50, // Greater than virtual_pool_count
        ..User::default()
    };
    pool.users.insert("0".to_string(), user);
    config.pools.insert("test_pool".to_string(), pool);

    // Set valid TLS rate limit
    config.general.tls_rate_limit_per_second = 100;

    // Set valid prepared statements config
    config.general.prepared_statements = true;
    config.general.prepared_statements_cache_size = 1024;

    // Validate should pass
    let result = config.validate().await;
    assert!(result.is_ok());
}

// Test pool_size less than virtual_pool_count
#[tokio::test]
async fn test_validate_pool_size_less_than_virtual_pool_count() {
    let mut config = Config::default();

    // Set virtual_pool_count to 10
    config.general.virtual_pool_count = 10;

    // Add a pool with a user whose pool_size is less than virtual_pool_count
    let mut pool = Pool::default();
    let user = User {
        username: "test_user".to_string(),
        password: "test_password".to_string(),
        pool_size: 5, // Less than virtual_pool_count
        ..User::default()
    };
    pool.users.insert("0".to_string(), user);
    config.pools.insert("test_pool".to_string(), pool);

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("pool_size"));
        assert!(msg.contains("virtual_pool_count"));
    } else {
        panic!("Expected BadConfig error about pool_size and virtual_pool_count");
    }
}

// Test TLS rate limit less than 100 (but not 0)
#[tokio::test]
async fn test_validate_tls_rate_limit_less_than_100() {
    let mut config = Config::default();

    // Set invalid TLS rate limit
    config.general.tls_rate_limit_per_second = 50;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls rate limit should be > 100"));
    } else {
        panic!("Expected BadConfig error about tls rate limit");
    }
}

// Test TLS rate limit not multiple of 100
#[tokio::test]
async fn test_validate_tls_rate_limit_not_multiple_of_100() {
    let mut config = Config::default();

    // Set invalid TLS rate limit
    config.general.tls_rate_limit_per_second = 150;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls rate limit should be multiple 100"));
    } else {
        panic!("Expected BadConfig error about tls rate limit multiple");
    }
}

// Test HBA and pg_hba both set
#[tokio::test]
async fn test_validate_hba_and_pg_hba_both_set() {
    let mut config = Config::default();

    // Set both HBA settings
    config.general.hba = vec!["192.168.1.0/24".parse().unwrap()];
    config.general.pg_hba = Some(crate::auth::hba::PgHba::default());

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("general.hba and general.pg_hba cannot be specified at the same time"));
    } else {
        panic!("Expected BadConfig error about hba and pg_hba");
    }
}

// Test prepared_statements enabled but cache_size is 0
#[tokio::test]
async fn test_validate_prepared_statements_no_cache() {
    let mut config = Config::default();

    // Set invalid prepared statements config
    config.general.prepared_statements = true;
    config.general.prepared_statements_cache_size = 0;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("prepared_statements_cache"));
    } else {
        panic!("Expected BadConfig error about prepared_statements_cache");
    }
}

// Test tls_certificate set but tls_private_key not set
#[tokio::test]
async fn test_validate_tls_certificate_without_private_key() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_certificate = Some("cert.pem".to_string());
    config.general.tls_private_key = None;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls_certificate is set but tls_private_key is not"));
    } else {
        panic!("Expected BadConfig error about tls_certificate without tls_private_key");
    }
}

// Test tls_private_key set but tls_certificate not set
#[tokio::test]
async fn test_validate_tls_private_key_without_certificate() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_certificate = None;
    config.general.tls_private_key = Some("key.pem".to_string());

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls_private_key is set but tls_certificate is not"));
    } else {
        panic!("Expected BadConfig error about tls_private_key without tls_certificate");
    }
}

// Test tls_mode set but tls_certificate or tls_private_key not set
#[tokio::test]
async fn test_validate_tls_mode_without_cert_or_key() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_mode = Some("require".to_string());
    config.general.tls_certificate = None;
    config.general.tls_private_key = None;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(
            msg.contains("tls_mode is require but tls_certificate or tls_private_key is not")
        );
    } else {
        panic!("Expected BadConfig error about tls_mode without cert/key");
    }
}

// Test tls_mode is verify-full but tls_ca_cert is not set
#[tokio::test]
async fn test_validate_tls_mode_verify_full_without_ca_cert() {
    let mut config = Config::default();

    // Set invalid TLS config
    config.general.tls_mode = Some("verify-full".to_string());
    config.general.tls_certificate = Some("cert.pem".to_string());
    config.general.tls_private_key = Some("key.pem".to_string());
    config.general.tls_ca_cert = None;

    // Validate should fail
    let result = config.validate().await;
    assert!(result.is_err());
    if let Err(Error::BadConfig(msg)) = result {
        assert!(msg.contains("tls_mode is verify-full but tls_ca_cert is not set"));
    } else {
        panic!("Expected BadConfig error about tls_mode verify-full without ca_cert");
    }
}

// Test valid TLS configuration with mode "allow"
#[tokio::test]
async fn test_validate_valid_tls_mode_allow() {
    let mut config = Config::default();

    // Set valid TLS config for "allow" mode
    config.general.tls_mode = Some("allow".to_string());

    // For "allow" mode, certificates are optional
    // Test without certificates to avoid certificate validation
    let result = config.validate().await;
    assert!(
        result.is_ok(),
        "Validation should pass for 'allow' mode without certificates"
    );
}

// Test valid TLS configuration with mode "disable"
#[tokio::test]
async fn test_validate_valid_tls_mode_disable() {
    let mut config = Config::default();

    // Set valid TLS config for "disable" mode
    config.general.tls_mode = Some("disable".to_string());

    // For "disable" mode, certificates are optional
    // Test without certificates to avoid certificate validation
    let result = config.validate().await;
    assert!(
        result.is_ok(),
        "Validation should pass for 'disable' mode without certificates"
    );
}
